//! Файл: tests/xt_deposit_min_t163.rs
//! Что делает: задача t163, решение 365 — минимум депозита XT берётся из поля depositMinAmount справочника
//! GET /v4/public/wallet/support/currency (https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetSupportedCurrencies:
//! «Minimum deposit amount»). Раньше поле не читалось и минимум был None без venue.deposit_min_absent_ok — направление
//! «… → XT» не торговалось (отказ «xt: минимальный депозит неизвестен — сделки нет», 2683 раза за 2 ч 16.11).
//! Проверки: пример ответа из документации (tests/data/xt_doc_2026-11-17) — минимум 1 при любом значении настройки;
//! «0» и отсутствие поля — по настройке (урок 21: смысл «0» не описан); живой ответ — #[ignore] до записи на сервере.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::xt::networks_parse. Моков нет.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::VenueId;
use arb::venues::xt::networks_parse;

/// Записанный ответ (JSON целиком, {rc, mc, ma, result}) → result.
fn result(path: &str) -> Value {
    let t = std::fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("нет записи {path} — tests/README.md, раздел XT"));
    let v: Value = serde_json::from_str(&t).expect("JSON");
    v.get("result").cloned().unwrap_or(v)
}

fn venue() -> VenueId {
    VenueId("xt".into())
}

/// Статусы монеты «ввод и вывод открыты» (в примере документации ответа /v4/public/currencies нет).
fn open(t: &str) -> HashMap<String, (bool, bool)> {
    HashMap::from([(t.to_string(), (true, true))])
}

#[test]
fn deposit_min_from_doc_example() {
    let data = result("tests/data/xt_doc_2026-11-17/support_currency_example.json");
    for absent_ok in [false, true] {
        let p = networks_parse::parse_networks(&data, &open("BTC"), "Bitcoin", &venue(), absent_ok)
            .unwrap();
        let btc = p
            .list
            .iter()
            .find(|i| i.asset.ticker == "BTC" && i.network == "Bitcoin")
            .expect("BTC / Bitcoin");
        assert_eq!(
            btc.deposit_min,
            Some(Decimal::ONE),
            "absent_ok = {absent_ok}"
        );
        assert_eq!(btc.withdraw_min, Some(Decimal::TEN));
    }
}

#[test]
fn zero_or_missing_deposit_min_follows_setting() {
    let base = result("tests/data/xt_doc_2026-11-17/support_currency_example.json");
    // Тот же пример документации, у сети поле «0» или убрано — смысл не описан (урок 21).
    let with = |v: Option<Value>| {
        let mut d = base.clone();
        let n = &mut d[0]["supportChains"][0];
        match v {
            Some(x) => n["depositMinAmount"] = x,
            None => {
                n.as_object_mut().unwrap().remove("depositMinAmount");
            }
        }
        d
    };
    for d in [with(Some(Value::from(0))), with(None)] {
        let get = |ok| {
            networks_parse::parse_networks(&d, &open("BTC"), "Bitcoin", &venue(), ok)
                .unwrap()
                .list[0]
                .deposit_min
        };
        assert_eq!(
            get(false),
            None,
            "не задано — минимум неизвестен, сделки нет"
        );
        assert_eq!(get(true), Some(Decimal::ZERO), "да — минимум 0");
    }
}

#[test]
#[ignore = "нужны записи cex_samples_2026_11_05/xt_support_currency.json и xt_currencies.json; ARB_XT_BSC — сеть BSC из записи"]
fn usdt_bsc_deposit_min_from_live_reply() {
    let dir = "tests/data/cex_samples_2026_11_05";
    let bsc = std::env::var("ARB_XT_BSC").expect("ARB_XT_BSC = название сети BSC из записи");
    let st = networks_parse::parse_statuses(&result(&format!("{dir}/xt_currencies.json"))).unwrap();
    let p = networks_parse::parse_networks(
        &result(&format!("{dir}/xt_support_currency.json")),
        &st,
        &bsc,
        &venue(),
        false,
    )
    .unwrap();
    let usdt = p
        .list
        .iter()
        .find(|i| i.asset.ticker == "USDT" && i.network == bsc)
        .expect("USDT в сети BSC");
    // Живой ответ 17.11.2026 (WebFetch): depositMinAmount USDT в BNB Smart Chain — 0.1.
    assert!(
        usdt.deposit_min.is_some_and(|m| m > Decimal::ZERO),
        "{:?}",
        usdt.deposit_min
    );
}
