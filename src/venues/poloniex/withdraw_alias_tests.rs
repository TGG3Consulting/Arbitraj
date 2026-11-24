//! Проверки сетевого имени Poloniex на проекциях валют и факта вывода из учёта.
//! Вызываются cargo test. См. tests/data/poloniex_withdraw_alias_2026-11-23/README.md.

use super::{Book, apply, resolve_or_lookup};
use crate::core::types::{Asset, CostKind, Transfer, TransferStatus};
use alloy_primitives::address;
use serde_json::Value;

const CURRENCIES: &str =
    include_str!("../../../tests/data/poloniex_withdraw_alias_2026-11-23/currencies.json");
const LEDGER: &str =
    include_str!("../../../tests/data/poloniex_withdraw_alias_2026-11-23/ledger.json");

/// Принятый вывод восстановлен из идентификатора истории и актива отправленного запроса.
/// Запись истории и зачисление действительно сохранены в учёте; ID обезличены одинаковым способом.
fn sample() -> (Transfer, Transfer, Transfer) {
    let d: Value = serde_json::from_str(LEDGER).unwrap();
    recorded(&d)
}

fn recorded(d: &Value) -> (Transfer, Transfer, Transfer) {
    let sent: Transfer = serde_json::from_value(d["sent"].clone()).unwrap();
    let credited: Transfer = serde_json::from_value(d["credited"].clone()).unwrap();
    let asset: Asset = serde_json::from_value(d["asset_submitted"].clone()).unwrap();
    let created = Transfer {
        asset,
        network: "BSC".into(),
        fee: None,
        tx_hash: None,
        status: TransferStatus::Pending,
        ..sent.clone()
    };
    (created, sent, credited)
}

/// Книга из опубликованных Poloniex валют, без приватных REST-запросов.
fn book() -> Book {
    let data: Vec<Value> = serde_json::from_str(CURRENCIES).unwrap();
    let mut book = Book::default();
    for c in data {
        book.insert(
            c["coin"].as_str().unwrap(),
            c["networkList"].as_array().unwrap(),
        );
    }
    book
}

#[test]
fn completed_bnb_withdrawal_is_the_same_coin_as_the_credit() {
    let (created, mut sent, credit) = sample();
    assert!(resolve_or_lookup(&created, &mut sent));
    assert!(book().confirms(&created, &sent.asset.ticker));
    let fee = sent.fee.as_ref().unwrap().amount;
    assert_eq!(sent.amount - fee, credit.amount);
    assert_eq!(sent.tx_hash, credit.tx_hash);
    apply(&created, &mut sent);
    assert_eq!(sent.asset, credit.asset);
    assert_eq!(sent.fee.as_ref().unwrap().asset, "BNB");
    assert_eq!(
        sent.amount - sent.fee.as_ref().unwrap().amount,
        credit.amount
    );
    assert!(!resolve_or_lookup(&created, &mut sent));
}

#[test]
fn alternate_network_unknown_alias_and_unrelated_contract_are_unconfirmed() {
    let (mut created, _, _) = sample();
    let book = book();
    assert!(!book.confirms(&created, "CELL-BSC"));
    created.network = "BNB".into();
    assert!(!book.confirms(&created, "BSC"));
    created.asset.ticker = "RACA".into();
    created.network = "BSC".into();
    assert!(!book.confirms(&created, "RACABSC")); // ещё контракт WBNB
    created.asset.contract = Some(address!("12bb890508c125661e03b09ec06e404bc9289040"));
    assert!(book.confirms(&created, "RACABSC"));
    created.network = "ETH".into(); // тот же адрес контракта в сети ETH
    assert!(!book.confirms(&created, "RACABSC"));
}

#[test]
fn ordinary_ticker_pending_unrelated_fee_or_wrong_id_never_require_a_lookup() {
    let (created, sent, credit) = sample();
    let mut standard = sent.clone();
    standard.asset.ticker = created.asset.ticker.clone();
    assert!(!resolve_or_lookup(&created, &mut standard));
    assert!(standard.asset.contract.is_none());
    let mut pending = sent.clone();
    pending.status = TransferStatus::Pending;
    assert!(!resolve_or_lookup(&created, &mut pending));
    let mut foreign = sent.clone();
    foreign.fee.as_mut().unwrap().asset = "USDT".into();
    assert!(!resolve_or_lookup(&created, &mut foreign));
    assert_eq!(foreign.fee.as_ref().unwrap().asset, "USDT");
    foreign.fee.as_mut().unwrap().asset = sent.asset.ticker.clone();
    foreign.fee.as_mut().unwrap().kind = CostKind::TransferGas;
    assert!(!resolve_or_lookup(&created, &mut foreign));
    let mut other_id = sent;
    other_id.venue_ref = credit.venue_ref;
    assert!(!resolve_or_lookup(&created, &mut other_id));
}

#[test]
fn sndk_history_keeps_the_submitted_contract_and_the_exchange_fee_units() {
    // Проекция выводов Poloniex от 24.11.2026. В истории контракт и сеть отсутствуют.
    // Контекст отправки восстановлен по venue_ticker::on_venue. ID биржи обезличены.
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json"
    ))
    .unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        let (created, mut sent, credit) = recorded(&row);
        assert!(sent.asset.contract.is_none());
        assert!(!resolve_or_lookup(&created, &mut sent));
        assert_eq!(sent.network, "BSC");
        assert_eq!(sent.asset, credit.asset); // совпали адрес и сеть, хотя тикеры разные
        assert_eq!(sent.asset.ticker, "SNDK");
        let fee = sent.fee.as_ref().unwrap();
        assert_eq!(fee.asset, sent.asset.ticker);
        assert_eq!(
            sent.amount - fee.amount,
            credit.amount * rust_decimal::Decimal::from(8)
        );
    }
}

#[test]
fn matching_ticker_does_not_override_contradictory_or_missing_identity() {
    let (mut created, mut sent, _) = sample();
    sent.asset.ticker = created.asset.ticker.clone();
    sent.fee.as_mut().unwrap().asset = created.asset.ticker.clone();
    sent.network = "ETH".into();
    assert!(!resolve_or_lookup(&created, &mut sent));
    assert!(sent.asset.contract.is_none());
    sent.network.clear();
    let wrong_contract = address!("12bb890508c125661e03b09ec06e404bc9289040");
    sent.asset.contract = Some(wrong_contract);
    assert!(!resolve_or_lookup(&created, &mut sent));
    assert_eq!(sent.asset.contract, Some(wrong_contract));
    sent.asset.contract = None;
    created.asset.contract = None;
    assert!(!resolve_or_lookup(&created, &mut sent));
    assert!(sent.asset.contract.is_none());
    assert!(sent.network.is_empty());
}
