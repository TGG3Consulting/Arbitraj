//! Файл: tests/bsc_dex_wallet_events.rs
//! Что делает: события баланса кошелька (решение 76) на ЖИВОЙ записи узла BNB Chain — квитанции блоков окна
//! tests/data/receipts_2026-10-25/responses.json (bsc-dataseed.bnbchain.org, 2026-10-25, записал техлид):
//! - фильтры «Transfer на кошелёк / с кошелька» (wallet/events_log.rs) через тот же разбор фильтров, что у ленты
//!   квитанций (node/log_filter.rs), выбирают ровно логи Transfer с адресом в поле to или from — посчитанные независимо;
//! - перевод 58,69 USDT в транзакции 0x3263…0835 (отправитель транзакции 0xa711…ff6a): для получателя — поступление
//!   не своей транзакцией (Deposit), для отправителя — списание своей транзакцией (не зачисление);
//! - сумма события подходит к ожидаемому приходу по допуску (core/credit_match.rs), к двум одинаковым — неоднозначно.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/receipts_common, arb::venues::bsc_dex::{node::{log_filter, receipts_chain}, wallet::{events_log,
//! events_tx}}, arb::core::credit_match.

mod receipts_common;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::credit_match::{Pick, fits, pick};
use arb::core::types::BalanceKind;
use arb::venues::bsc_dex::node::log_filter::Filter;
use arb::venues::bsc_dex::node::receipts_chain::logs_of;
use arb::venues::bsc_dex::wallet::events_log::{filters, parse};
use arb::venues::bsc_dex::wallet::events_tx::kind;
use receipts_common::{load, result};

const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const TX: &str = "0x3263576c6e5102aa5d25fcfbf93717423687652f2a6dbd04d205523318f20835";
const SENDER: &str = "0xa71180a5bfbc0dbd18f7b355e4afa101a319ff6a";
const RECIPIENT: &str = "0x7f179f0808d7c43f750d154e026c07c069ae97ba";
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

/// Квитанции блока 129702236 и их логи.
fn block() -> (Vec<Value>, Vec<Value>) {
    let rec = load("responses.json");
    let receipts = result(&rec, "receipts 129702236")
        .as_array()
        .unwrap()
        .clone();
    let logs = logs_of(&receipts);
    (receipts, logs)
}

/// Отправитель транзакции из квитанции (поле from квитанции = from транзакции).
fn sender_of(receipts: &[Value], tx: &str) -> Address {
    let r = receipts
        .iter()
        .find(|r| r["transactionHash"] == tx)
        .unwrap();
    r["from"].as_str().unwrap().parse().unwrap()
}

/// Логи, прошедшие фильтры кошелька.
fn picked(logs: &[Value], wallet: Address) -> Vec<Value> {
    let fs: Vec<Filter> = filters(wallet).iter().map(Filter::new).collect();
    logs.iter()
        .filter(|l| fs.iter().any(|f| f.matches(l)))
        .cloned()
        .collect()
}

#[test]
fn filters_pick_exactly_wallet_transfers() {
    let (_, logs) = block();
    for w in [SENDER, RECIPIENT] {
        let wallet: Address = w.parse().unwrap();
        let pad = format!("0x{:0>64}", &w[2..]);
        // Независимо: topic0 Transfer и адрес кошелька в from (topics[1]) или to (topics[2]).
        let want = logs
            .iter()
            .filter(|l| {
                let t = l["topics"].as_array().unwrap();
                t.first().and_then(Value::as_str) == Some(TRANSFER)
                    && (t.get(1).and_then(Value::as_str) == Some(pad.as_str())
                        || t.get(2).and_then(Value::as_str) == Some(pad.as_str()))
            })
            .count();
        let got = picked(&logs, wallet);
        assert!(want > 0, "в блоке есть переводы {w}");
        assert_eq!(got.len(), want, "{w}");
    }
}

#[test]
fn recipient_sees_deposit_sender_sees_own_outflow() {
    let (receipts, logs) = block();
    let units = U256::from(58_690_000_000_000_000_000u128);
    let from_tx = sender_of(&receipts, TX);
    // Получатель: поступление не своей транзакцией — Deposit.
    let r: Address = RECIPIENT.parse().unwrap();
    let mv: Vec<_> = picked(&logs, r)
        .iter()
        .filter_map(|l| parse(&serde_json::from_value::<Log>(l.clone()).unwrap(), r).unwrap())
        .filter(|m| m.tx.to_string() == TX)
        .collect();
    assert_eq!(mv.len(), 1);
    let m = &mv[0];
    assert!(m.inbound && !m.removed);
    assert_eq!(m.units, units);
    assert_eq!(m.token, USDT.parse::<Address>().unwrap());
    assert_eq!(m.block, 129702236);
    let own = from_tx == r;
    assert!(!own);
    assert_eq!(kind(m.inbound, own), BalanceKind::Deposit);
    // Отправитель: списание своей транзакцией — не зачисление.
    let s: Address = SENDER.parse().unwrap();
    let out: Vec<_> = picked(&logs, s)
        .iter()
        .filter_map(|l| parse(&serde_json::from_value::<Log>(l.clone()).unwrap(), s).unwrap())
        .filter(|m| m.tx.to_string() == TX)
        .collect();
    assert_eq!(out.len(), 1);
    assert!(!out[0].inbound);
    assert_eq!(out[0].units, units);
    assert_eq!(from_tx, s);
    assert_eq!(kind(out[0].inbound, from_tx == s), BalanceKind::Other);
}

#[test]
fn amount_matches_expected_within_tolerance() {
    let got: Decimal = "58.69".parse().unwrap();
    let zero = Decimal::ZERO;
    assert!(fits(got, got, zero));
    assert_eq!(pick(&[got], got, zero), Pick::One(0));
    // Другой ожидаемый приход — не подходит; два одинаковых — неоднозначно (решает опрос статуса).
    let other: Decimal = "58.70".parse().unwrap();
    assert_eq!(pick(&[other], got, zero), Pick::None);
    assert_eq!(pick(&[other, got], got, zero), Pick::One(1));
    assert_eq!(pick(&[got, got], got, zero), Pick::Many(2));
}
