//! Файл: tests/core_market_signal.rs
//! Что делает: проверяет состояние рынка ядра и сигнал на реально записанных стаканах BingX
//! (tests/data/bingx, запись 2026-10-23): применение стаканов, лучшая цена, худшая цена объёма,
//! сброс недостоверных данных; грубый спред на реальных ценах; чёрный список без проверок.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::bingx::depth (только чтобы разобрать запись). Моков нет.
//!
//! В записи — одна площадка и по одному стакану TWT и CAKE, поэтому «кандидат» по ним невозможен:
//! проверяется то, что реальные данные позволяют (спред на одной площадке отрицательный → «ниже порога»).

use std::str::FromStr;

use arb::core::blacklist::{self, TaxLimits};
use arb::core::market_state::{MarketState, Touched};
use arb::core::signal::{Params, Prices, Signals, State};
use arb::core::types::{Asset, Direction, OrderBook, Side, VenueId};
use arb::gateway::events::MarketEvent;
use arb::venues::bingx::depth::{self, Msg};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Стаканы записи в формате ядра (тикер — символ без «-USDT», как его отдаёт адаптер BingX).
fn books() -> Vec<OrderBook> {
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    raw.lines()
        .filter_map(|l| {
            let m = serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string();
            match depth::parse(&m) {
                Msg::Depth {
                    symbol,
                    bids,
                    asks,
                    exchange_time,
                } => Some(OrderBook {
                    venue: VenueId("bingx".into()),
                    asset: Asset {
                        ticker: symbol.trim_end_matches("-USDT").to_string(),
                        bsc_contract: None,
                    },
                    bids,
                    asks,
                    exchange_time,
                    received_at: Utc::now(),
                }),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn market_state_books() {
    let mut m = MarketState::default();
    let bx = VenueId("bingx".into());
    for b in books() {
        assert!(matches!(
            m.apply(MarketEvent::Book(b)),
            Touched::Book { first: true, .. }
        ));
    }
    assert_eq!(m.best(&bx, "CAKE", Side::Buy), Some(d("2.189")));
    assert_eq!(m.best(&bx, "CAKE", Side::Sell), Some(d("2.188")));
    assert_eq!(m.best(&bx, "TWT", Side::Buy), Some(d("0.4578")));
    // 100 CAKE: 11.68 по 2.189 и остаток по 2.190 — худшая цена лимитного ордера 2.190.
    assert_eq!(
        m.deepest_price(&bx, "CAKE", Side::Buy, d("100")),
        Some(d("2.190"))
    );
    // Объём больше всей глубины — цены нет (ордер целиком не исполнится).
    let all: Decimal = m
        .book(&bx, "CAKE")
        .unwrap()
        .asks
        .iter()
        .map(|l| l.qty)
        .sum();
    assert_eq!(m.deepest_price(&bx, "CAKE", Side::Buy, all + d("1")), None);
    m.apply(MarketEvent::Invalidated {
        venue: bx.clone(),
        ticker: "CAKE".into(),
    });
    assert!(m.book(&bx, "CAKE").is_none());
    assert!(m.book(&bx, "TWT").is_some());
}

#[test]
fn signal_on_real_prices() {
    let b = books()
        .into_iter()
        .find(|b| b.asset.ticker == "CAKE")
        .unwrap();
    let dir = Direction {
        asset: b.asset.clone(),
        buy_on: b.venue.clone(),
        sell_on: b.venue.clone(),
    };
    let prices = Prices {
        buy: b.asks[0].price,
        sell: b.bids[0].price,
        fees_pct: Decimal::ZERO,
        qty: b.asks[0].qty,
    };
    let params: Result<Params, String> = Err("пороги не нужны: спред ниже комиссий".into());
    let mut s = Signals::default();
    let v = s.update(&dir, Ok(prices), &params, Utc::now());
    assert_eq!(v.state, State::Below);
    assert!(v.spread_pct.unwrap() < Decimal::ZERO);
    let v = s.update(&dir, Err("нет котировки".into()), &params, Utc::now());
    assert_eq!(v.state, State::NoPrice("нет котировки".into()));
}

#[test]
fn blacklist_without_checks() {
    let no_limits: Result<TaxLimits, String> = Err("не заданы".into());
    // Монета не на DEX и не отмечена вручную — в чёрном списке её нет.
    assert_eq!(blacklist::reason(None, &[], &no_limits), Ok(None));
    // На DEX есть пул, но проверки налогов ещё не было — торговать нельзя.
    assert!(matches!(
        blacklist::reason(None, &[None], &no_limits),
        Ok(Some(arb::core::types::BlockReason::NotChecked))
    ));
}
