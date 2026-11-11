//! Файл: tests/order_type_filter.rs
//! Что делает: решение 254 — пара, на которой биржа не примет наш ордер (LIMIT с нужным timeInForce / типом), отсеивается
//! ДО расчёта, по справочнику пар биржи, на ЖИВЫХ записях:
//! - XT (tests/data/xt_live_2026-11-06/symbol_pepe.json, 06.11.2026): pepe_usdt — orderTypes [] и timeInForces [] →
//!   признак order_block с причиной, в торгуемые (SpotPairs.tickers) не входит, стакан подписывается; btc_usdt —
//!   orderTypes [LIMIT, MARKET], timeInForces [GTC, IOC] → при IOC признака нет, при FOK — есть («FOK нет в
//!   timeInForces»); причина та же, что у проверки перед ордером (order_rules::check_rules — страховка);
//! - ядро (core/pairs_fresh.rs — сторона сделки): по паре с признаком нельзя ни купить, ни продать; обычная — можно;
//! - MEXC (tests/data/mexc_2026-11-02/exchange_info.json): у всех пар LIMIT есть → IOC (решение 133) признака не даёт,
//!   тип не из orderTypes (STOP_LOSS) — даёт.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::{xt, mexc}, arb::core (market_state, pairs_fresh). Моков нет.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::market_state::MarketState;
use arb::core::pairs_fresh;
use arb::core::types::{Asset, Side, VenueId};
use arb::venues::xt::pairs::Snapshot;
use arb::venues::xt::{order_rules, symbols, symbols_parse};

fn json(path: &str) -> Value {
    let p = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Время ответа XT в записи (поле time, мс).
fn xt_at(v: &Value) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(v["time"].as_i64().unwrap()).unwrap()
}

/// Справочник пар XT из живой записи 06.11.2026 и время ответа.
fn xt() -> (symbols::Symbols, DateTime<Utc>) {
    let v = json("tests/data/xt_live_2026-11-06/symbol_pepe.json");
    let map = symbols_parse::parse_list(&v, "USDT").unwrap().map.unwrap();
    let s = symbols::Symbols::default();
    let at = xt_at(&v);
    s.load(map, at);
    (s, at)
}

fn assets() -> Vec<Asset> {
    ["BTC", "PEPE"]
        .iter()
        .map(|t| Asset {
            ticker: t.to_string(),
            bsc_contract: None,
        })
        .collect()
}

fn pair_block(snap: &Snapshot, t: &str) -> Option<String> {
    snap.pairs
        .iter()
        .find(|p| p.ticker == t)
        .unwrap()
        .order_block
        .clone()
}

#[test]
fn xt_pair_without_limit_is_marked() {
    let (s, _) = xt();
    let snap = Snapshot::build(&s, &assets(), Some("IOC"));
    let pepe = pair_block(&snap, "PEPE").expect("у pepe_usdt orderTypes [] — признак есть");
    assert_eq!(pepe, "pepe_usdt: типа LIMIT нет в orderTypes пары");
    assert_eq!(pair_block(&snap, "BTC"), None, "btc_usdt: LIMIT и IOC есть");
    assert_eq!(snap.tickers, ["BTC"], "PEPE — не торгуемая нашим ордером");
    assert!(
        snap.books.iter().any(|(a, _)| a.ticker == "PEPE"),
        "стакан PEPE подписан"
    );
    assert_eq!(snap.order_blocked(), std::slice::from_ref(&pepe));
    // Та же причина у проверки перед ордером (страховка).
    let v = json("tests/data/xt_live_2026-11-06/symbol_pepe.json");
    let map = symbols_parse::parse_list(&v, "USDT").unwrap().map.unwrap();
    let r = order_rules::check_rules(
        &map["PEPE"],
        Side::Buy,
        Decimal::from(1000),
        Decimal::ONE,
        "IOC",
    );
    assert_eq!(r.unwrap_err(), pepe);
}

#[test]
fn xt_time_in_force_from_setting() {
    let (s, _) = xt();
    let fok = Snapshot::build(&s, &assets(), Some("FOK"));
    assert_eq!(
        pair_block(&fok, "BTC").as_deref(),
        Some("btc_usdt: FOK нет в timeInForces пары")
    );
    assert!(fok.tickers.is_empty());
    // Настройка не задана — сверяется только LIMIT (ордер без настройки и так не уйдёт: NotConfigured).
    let none = Snapshot::build(&s, &assets(), None);
    assert_eq!(pair_block(&none, "BTC"), None);
    assert!(pair_block(&none, "PEPE").is_some());
    // Смена настройки — видимое изменение снимка (pairs_watch.rs шлёт ядру новый SpotPairs и пишет журнал).
    let ioc = Snapshot::build(&s, &assets(), Some("IOC"));
    let ch = fok.changes(&ioc);
    assert_eq!(ch.len(), 1, "{ch:?}");
    assert!(ch[0].starts_with("BTC: API: покупка да, продажа да → не примет наш ордер"));
}

#[test]
fn core_skips_both_sides_of_blocked_pair() {
    let (s, at) = xt();
    let xt_id = VenueId("xt".into());
    let mut m = MarketState::default();
    m.apply(Snapshot::build(&s, &assets(), Some("IOC")).event(xt_id.clone(), at));
    let limit = Ok(60);
    for buy in [true, false] {
        let e = pairs_fresh::side(&m, &xt_id, "PEPE", buy, &limit, at).unwrap_err();
        assert_eq!(
            e,
            "xt: биржа не примет наш ордер на паре PEPE: pepe_usdt: типа LIMIT нет в orderTypes пары"
        );
        assert!(pairs_fresh::side(&m, &xt_id, "BTC", buy, &limit, at).is_ok());
    }
}

#[test]
fn mexc_order_type_by_order_types() {
    use arb::venues::mexc::{order_rules as mexc_rules, symbols as mexc_symbols};
    let v = json("tests/data/mexc_2026-11-02/exchange_info.json");
    let allowed: HashSet<String> = ["CAKEUSDT".to_string()].into();
    let map = mexc_symbols::parse_list(&v, "USDT", &allowed).unwrap();
    let cake = &map["CAKE"];
    for t in ["IMMEDIATE_OR_CANCEL", "FILL_OR_KILL", "LIMIT"] {
        assert_eq!(mexc_rules::order_block(cake, Some(t)), None, "{t}");
    }
    assert_eq!(mexc_rules::order_block(cake, None), None);
    assert_eq!(
        mexc_rules::order_block(cake, Some("STOP_LOSS")).as_deref(),
        Some(
            "CAKEUSDT: тип ордера STOP_LOSS (venue.order_type) не в orderTypes пары (LIMIT, MARKET, LIMIT_MAKER)"
        )
    );
}
