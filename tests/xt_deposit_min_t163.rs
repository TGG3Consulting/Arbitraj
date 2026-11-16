//! Файл: tests/xt_deposit_min_t163.rs
//! Что делает: задача t163, решение 365 — минимум депозита XT берётся из поля depositMinAmount справочника
//! GET /v4/public/wallet/support/currency (https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetSupportedCurrencies:
//! «Minimum deposit amount»). Раньше поле не читалось и минимум был None без venue.deposit_min_absent_ok — направление
//! «… → XT» не торговалось (отказ «xt: минимальный депозит неизвестен — сделки нет», 2683 раза за 2 ч 16.11).
//! Проверки: пример ответа из документации (tests/data/xt_doc_2026-11-17) — минимум 1 при любом значении настройки;
//! «0» и отсутствие поля — по настройке (урок 21: смысл «0» не описан); живой ответ бота 16.11
//! (tests/data/xt_live_2026-11-16): USDT 0.1, USDC 0.01, WIN «0», из 272 сетей BSC минимум > 0 у 6.
//! Ревью t163 Н1: строкой "0.1" — минимум; "" и "0" строкой, не число — как нет поля, сеть (и вывод) остаётся, причина —
//! в пропущенных/ограниченных (журнал); Н3: depositConfirmations примера — 2.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::xt::networks_parse. Моков нет.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::VenueId;
use arb::core::usdt_transfer_check::deposit_min_known;
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

/// Живой ответ из записи бота 16.11 (tests/data/xt_live_2026-11-16, zstd) → result.
fn live(file: &str) -> Value {
    let p = format!("tests/data/xt_live_2026-11-16/{file}");
    let raw = zstd::decode_all(std::fs::File::open(&p).expect("запись")).expect("zstd");
    let v: Value = serde_json::from_slice(&raw).expect("JSON");
    v.get("result").cloned().unwrap_or(v)
}

#[test]
fn deposit_min_from_live_reply_bnb_smart_chain() {
    // venue.bsc_network [xt] на живом боте — «BNB Smart Chain» (выгрузка настроек 16.11).
    let bsc = "BNB Smart Chain";
    let st = networks_parse::parse_statuses(&live("currencies.json.zst")).unwrap();
    let support = live("support_currency.json.zst");
    let get = |ok: bool| networks_parse::parse_networks(&support, &st, bsc, &venue(), ok).unwrap();
    let off = get(false);
    let on = |p: &networks_parse::Parsed, t: &str| {
        p.list
            .iter()
            .find(|i| i.asset.ticker == t && i.network == bsc)
            .cloned()
            .unwrap_or_else(|| panic!("{t} в сети {bsc}"))
    };
    // Живой бот (venue.deposit_min_absent_ok [xt] = нет): у USDT и USDC минимум известен — перевод на XT считается.
    assert_eq!(on(&off, "USDT").deposit_min, Some(Decimal::new(1, 1)));
    assert_eq!(on(&off, "USDC").deposit_min, Some(Decimal::new(1, 2)));
    assert_eq!(on(&off, "USDT").deposit_confirmations, Some(15));
    // WIN — «0»: смысл не описан, решает человек.
    assert_eq!(on(&off, "WIN").deposit_min, None);
    assert_eq!(on(&get(true), "WIN").deposit_min, Some(Decimal::ZERO));
    // Сколько сетей BSC получили минимум из ответа (> 0) — 6 из 272 (остальные «0»).
    let bsc_nets: Vec<_> = off.list.iter().filter(|i| i.network == bsc).collect();
    let known = bsc_nets.iter().filter(|i| i.deposit_min.is_some()).count();
    assert_eq!((known, bsc_nets.len()), (6, 272));
    // Все сети разобраны: depositMinAmount — число у всех, ни одна сеть не выброшена из-за него.
    assert!(
        !off.skipped.iter().any(|s| s.contains("depositMinAmount")),
        "{:?}",
        off.skipped.iter().take(3).collect::<Vec<_>>()
    );
    // Ревью Н5: перевод USDT вручную на XT — минимум известен; у монеты с «0» без решения человека — отказ.
    let usdt = on(&off, "USDT");
    assert_eq!(
        deposit_min_known("xt", "USDT", bsc, usdt.deposit_min),
        Ok(Decimal::new(1, 1))
    );
    assert!(deposit_min_known("xt", "WIN", bsc, on(&off, "WIN").deposit_min).is_err());
}

#[test]
fn string_and_unreadable_deposit_min_keep_network() {
    let base = result("tests/data/xt_doc_2026-11-17/support_currency_example.json");
    let parse = |v: Value, ok: bool| {
        let mut d = base.clone();
        d[0]["supportChains"][0]["depositMinAmount"] = v;
        networks_parse::parse_networks(&d, &open("BTC"), "Bitcoin", &venue(), ok).unwrap()
    };
    let p = parse(Value::from("0.1"), false);
    assert_eq!(p.list[0].deposit_min, Some(Decimal::new(1, 1)));
    for v in [Value::from(""), Value::from("0"), Value::from("abc")] {
        for ok in [false, true] {
            let p = parse(v.clone(), ok);
            assert_eq!(p.list.len(), 1, "{v}: сеть не выброшена");
            let n = &p.list[0];
            assert!(n.withdraw_enabled, "{v}: вывод с XT остаётся");
            assert_eq!(
                n.deposit_min,
                ok.then_some(Decimal::ZERO),
                "{v}, absent_ok = {ok}"
            );
        }
    }
    let p = parse(Value::from(""), false);
    assert!(
        p.skipped.iter().any(|s| s.contains("depositMinAmount")),
        "причина — в журнал: {:?}",
        p.skipped
    );
}

#[test]
fn deposit_confirmations_from_doc_example() {
    let data = result("tests/data/xt_doc_2026-11-17/support_currency_example.json");
    let p =
        networks_parse::parse_networks(&data, &open("BTC"), "Bitcoin", &venue(), false).unwrap();
    assert_eq!(p.list[0].deposit_confirmations, Some(2));
}
