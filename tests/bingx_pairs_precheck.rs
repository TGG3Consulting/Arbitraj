//! Файл: tests/bingx_pairs_precheck.rs
//! Что делает: предпроверка стороны сделки по справочнику пар (core/pairs_fresh.rs::side — её вызывает precheck.rs)
//! и строгий разбор apiStateBuy/apiStateSell (venues/bingx/symbols.rs) на живом ответе common/symbols 2026-10-24
//! (tests/data/bingx_2026-10-24/symbols.json):
//! - TAG (status 1, API закрыт): ни купить на BingX, ни продать; CAKE — обе стороны; SEA — status 10;
//! - справочник старше venue.symbols_stale_s или порог не задан — сделки нет;
//! - в паре нет apiStateBuy или он не boolean — BadResponse на весь справочник, а не «нельзя» по догадке.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (pairs_fresh, market_state), arb::venues::bingx (symbols, pairs). Моков нет.

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use arb::core::market_state::MarketState;
use arb::core::pairs_fresh::side;
use arb::core::types::{Asset, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::symbols;

/// Время ответа symbols в сыром журнале (мс).
const SYMBOLS_AT_MS: i64 = 1792868538956;

fn bingx() -> VenueId {
    VenueId("bingx".into())
}

fn response() -> Value {
    let raw = std::fs::read_to_string("tests/data/bingx_2026-10-24/symbols.json").unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// Состояние рынка после SpotPairs по записанному ответу; монеты подписки — тикеры (адрес тут не нужен).
fn market(at: DateTime<Utc>) -> MarketState {
    let s = symbols::Symbols::default();
    s.load(
        symbols::parse_list(&response()["data"], "USDT").unwrap(),
        at,
    );
    let assets: Vec<Asset> = ["TAG", "CAKE", "SEA"]
        .iter()
        .map(|t| Asset {
            ticker: t.to_string(),
            bsc_contract: None,
        })
        .collect();
    let mut m = MarketState::default();
    m.apply(Snapshot::build(&s, &assets).event(bingx(), at));
    m
}

#[test]
fn side_by_live_flags() {
    let at = DateTime::from_timestamp_millis(SYMBOLS_AT_MS).unwrap();
    let m = market(at);
    let now = at + Duration::seconds(60);
    let ok = Ok(120);
    let buy_tag = side(&m, &bingx(), "TAG", true, &ok, now).unwrap_err();
    assert!(buy_tag.contains("apiStateBuy"), "{buy_tag}");
    let sell_tag = side(&m, &bingx(), "TAG", false, &ok, now).unwrap_err();
    assert!(sell_tag.contains("apiStateSell"), "{sell_tag}");
    assert!(side(&m, &bingx(), "CAKE", true, &ok, now).is_ok());
    assert!(side(&m, &bingx(), "CAKE", false, &ok, now).is_ok());
    let sea = side(&m, &bingx(), "SEA", true, &ok, now).unwrap_err();
    assert!(sea.contains("status 10"), "{sea}");
    // Справочник старше порога (обновление не проходит) — сделки нет даже по открытой паре.
    let old = side(&m, &bingx(), "CAKE", true, &Ok(30), now).unwrap_err();
    assert!(old.contains("устарел"), "{old}");
    // Порог не задан — сделки нет.
    let unset = Err("не задана настройка venue.symbols_stale_s".to_string());
    assert!(side(&m, &bingx(), "CAKE", true, &unset, now).is_err());
}

#[test]
fn missing_api_flag_is_bad_response() {
    for (field, value) in [
        ("apiStateBuy", None),
        ("apiStateSell", Some(Value::from(1))),
    ] {
        let mut v = response();
        let tag = v["data"]["symbols"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|s| s["symbol"] == "TAG-USDT")
            .unwrap();
        match value {
            None => {
                tag.as_object_mut().unwrap().remove(field);
            }
            Some(x) => tag[field] = x,
        }
        let e = symbols::parse_list(&v["data"], "USDT").unwrap_err();
        assert_eq!(e.code, ErrorCode::BadResponse);
        assert!(e.message.contains(field), "{}", e.message);
    }
    // Живой ответ как есть — разбирается.
    assert!(symbols::parse_list(&response()["data"], "USDT").is_ok());
}
