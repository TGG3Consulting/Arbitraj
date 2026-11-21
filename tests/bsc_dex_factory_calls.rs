//! Файл: tests/bsc_dex_factory_calls.rs
//! Что делает: поиск пулов по фабрикам (решение 121, C3) на записанных ответах узла BSC 2026-10-24: данные вызова
//! getPair (PancakeSwap V2) и getPool (PancakeSwap V3, fee 2500) совпадают с записанным запросом байт в байт, ответ
//! фабрики разбирается в адрес пула из записи; сумма пополнения сухой сделки (C4) с налогом токена.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::factory_calls, arb::venues::bsc_dex::exec::dry_fund,
//! tests/data/indexers_2026-10-24/arb_node_responses{,_2}_2026-10-24.json.
//! Настройки: нет.

use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::TokenTaxes;
use arb::venues::bsc_dex::exec::dry_fund;
use arb::venues::bsc_dex::pools::factory_calls::{decode, encode};

/// Записанный вызов по имени: (входные данные, ответ). Ошибок нет (тест падает, если записи нет).
fn rec(file: &str, name: &str) -> (Bytes, Bytes) {
    let text = std::fs::read_to_string(file).unwrap();
    let all: Value = serde_json::from_str(&text).unwrap();
    let list = all["results"].as_array().unwrap();
    let x = list.iter().find(|x| x["name"] == name).unwrap();
    let p = &x["request"]["params"][0];
    (
        p["input"].as_str().unwrap().parse().unwrap(),
        x["response"]["result"].as_str().unwrap().parse().unwrap(),
    )
}

/// Адрес из текста записи. Ошибок нет.
fn a(s: &str) -> Address {
    s.parse().unwrap()
}

const CAKE: &str = "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82";
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";
const WBNB: &str = "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c";

#[test]
fn v2_get_pair_as_recorded() {
    let (input, out) = rec(
        "tests/data/indexers_2026-10-24/arb_node_responses_2026-10-24.json",
        "CAKE pancakeswap_v2 0x0ed7e52944161450477ee417de9cd3a859b14fd0 PancakeSwap V2 getPair",
    );
    assert_eq!(encode(a(CAKE), a(WBNB), None), input);
    let pool = a("0x0ed7e52944161450477ee417de9cd3a859b14fd0");
    assert_eq!(decode(None, &out).unwrap(), pool);
}

#[test]
fn v3_get_pool_as_recorded() {
    let (input, out) = rec(
        "tests/data/indexers_2026-10-24/arb_node_responses_2_2026-10-24.json",
        "CAKE pancakeswap-v3-bsc 0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1 getPool",
    );
    // fee 0x09c4 = 2500 — из записанного запроса.
    assert_eq!(encode(a(CAKE), a(USDT), Some(2500)), input);
    let pool = a("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1");
    assert_eq!(decode(Some(2500), &out).unwrap(), pool);
    // Ответ не по ABI — текст, а не паника.
    assert!(decode(Some(2500), &out[..10]).is_err());
}

#[test]
fn funding_gross_with_tax() {
    let n = U256::from(1000u32);
    assert_eq!(dry_fund::gross(n, None), Some(n));
    let t = TokenTaxes {
        buy_pct: Decimal::from(3),
        sell_pct: Decimal::from(9),
        transfer_pct: Decimal::from(5),
    };
    // Наибольший из налога покупки и перевода (5 %): 1000 / 0,95 = 1052,6… → 1053 вверх.
    assert_eq!(dry_fund::gross(n, Some(&t)), Some(U256::from(1053u32)));
    let all = TokenTaxes {
        buy_pct: Decimal::ONE_HUNDRED,
        ..t
    };
    assert_eq!(dry_fund::gross(n, Some(&all)), None);
}
