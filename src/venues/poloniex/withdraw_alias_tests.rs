//! Проверки сетевого имени Poloniex на проекциях валют и факта вывода из учёта.
//! Вызываются cargo test. См. tests/data/poloniex_withdraw_alias_2026-11-23/README.md.

use super::{Book, apply, needs_alias};
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
    assert!(needs_alias(&created, &sent));
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
    assert!(!needs_alias(&created, &sent));
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
    assert!(!needs_alias(&created, &standard));
    let mut pending = sent.clone();
    pending.status = TransferStatus::Pending;
    assert!(!needs_alias(&created, &pending));
    let mut foreign = sent.clone();
    foreign.fee.as_mut().unwrap().asset = "USDT".into();
    assert!(!needs_alias(&created, &foreign));
    foreign.fee.as_mut().unwrap().asset = sent.asset.ticker.clone();
    foreign.fee.as_mut().unwrap().kind = CostKind::TransferGas;
    assert!(!needs_alias(&created, &foreign));
    let mut other_id = sent;
    other_id.venue_ref = credit.venue_ref;
    assert!(!needs_alias(&created, &other_id));
}
