//! Файл: tests/core_book_marks_317.rs
//! Что делает: решение 317, ревью 16.11 — отметки изменения стакана в состоянии рынка ядра (core/book_still.rs через
//! MarketState, core/book_marks_on.rs). Чистая логика, время — параметром (без sleep):
//! (1) вставка и удаление уровня в середине стакана двигают отметки только префиксов, которые его содержат;
//! (2) страховка (б): кадр позже прошлого больше чем на порог свежести стакана площадки (venue.book_stale_ms) —
//! отсчёт заново; ровно на пороге — нет;
//! (3) страховка (а): котировка по более новому кадру, чем стакан ядра (кадр не дошёл до ядра), — «стоит» не
//! доказано (None) → обычное удержание;
//! (4) признак выключен — отметок нет (note не вызывается); включили — отсчёт с первого кадра после включения, даже
//! если содержимое не менялось; выключили — отметки очищены.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::market_state, arb::core::signal, arb::core::signal_fast, arb::core::types,
//! arb::venues::bingx::depth (только чтобы разобрать запись). Моков нет: стакан — реально записанный кадр BingX
//! CAKE-USDT@depth20 (tests/data/bingx/ws_spot_2026-10-23.jsonl); изменённые кадры — тот же кадр с изменённым уровнем.

use std::collections::HashMap;
use std::str::FromStr;

use arb::core::market_state::MarketState;
use arb::core::signal::{Params, Prices, Signals, State};
use arb::core::signal_fast::{Ctx, Trigger};
use arb::core::types::{Asset, Direction, OrderBook, PriceLevel, Side, VenueId};
use arb::gateway::events::MarketEvent;
use arb::venues::bingx::depth::{self, Msg};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn at(t0: DateTime<Utc>, ms: i64) -> DateTime<Utc> {
    t0 + Duration::milliseconds(ms)
}

fn bingx() -> VenueId {
    VenueId("bingx".into())
}

/// Реально записанный стакан BingX CAKE (20 уровней), получен в `received`.
fn cake(received: DateTime<Utc>) -> OrderBook {
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    raw.lines()
        .find_map(|l| {
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
                } if symbol == "CAKE-USDT" => Some(OrderBook {
                    venue: bingx(),
                    asset: Asset {
                        ticker: "CAKE".into(),
                        bsc_contract: None,
                    },
                    bids,
                    asks,
                    exchange_time,
                    received_at: received,
                }),
                _ => None,
            }
        })
        .unwrap()
}

/// Состояние рынка с включёнными отметками и порогами свежести стакана по площадке.
fn marked(gaps: &[(&str, u64)]) -> MarketState {
    let mut m = MarketState::default();
    let g: HashMap<VenueId, u64> = gaps
        .iter()
        .map(|(v, ms)| (VenueId((*v).into()), *ms))
        .collect();
    m.set_book_marks(true, g);
    m
}

/// С какого момента стоят покупатели (продажа на бирже) на объёме qty при котировке по кадру quote_at.
fn since_q(m: &MarketState, qty: &str, quote_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    m.book_still_since(&bingx(), "CAKE", Side::Sell, d(qty), quote_at)
}

/// То же, котировка по кадру не новее стакана ядра.
fn since(m: &MarketState, qty: &str) -> Option<DateTime<Utc>> {
    since_q(m, qty, DateTime::<Utc>::MIN_UTC)
}

#[test]
fn middle_level_insert_and_delete() {
    // Покупатели записи: 12.903, 1089.137, 849.992, … — объём 1100 покрывают первые 2 уровня, 1103 — 3 (после
    // вставки уровня 1 монета между вторым и третьим).
    let mut m = marked(&[]);
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    let mut b = cake(at(t0, 400));
    let mid = (b.bids[1].price + b.bids[2].price) / Decimal::TWO;
    b.bids.insert(
        2,
        PriceLevel {
            price: mid,
            qty: d("1"),
        },
    );
    m.apply(MarketEvent::Book(b));
    assert_eq!(since(&m, "10"), Some(t0));
    assert_eq!(since(&m, "1100"), Some(t0));
    assert_eq!(since(&m, "1103"), Some(at(t0, 400)));
    // Уровень удалён (стакан как в записи) — снова изменение с третьего уровня.
    m.apply(MarketEvent::Book(cake(at(t0, 800))));
    assert_eq!(since(&m, "1100"), Some(t0));
    assert_eq!(since(&m, "1103"), Some(at(t0, 800)));
    // Удалён исходный третий уровень: префиксы с ним и глубже — с этого кадра, первые два — нет.
    let mut c = cake(at(t0, 1200));
    c.bids.remove(2);
    m.apply(MarketEvent::Book(c));
    assert_eq!(since(&m, "1100"), Some(t0));
    assert_eq!(since(&m, "1103"), Some(at(t0, 1200)));
}

#[test]
fn gap_over_book_stale_restarts() {
    // Порог свежести стакана bingx 1000 мс: тот же стакан через 900 и ещё через 1000 мс (ровно порог) — отметка t0;
    // следующий кадр через 1100 мс — ядро не наблюдало рынок дольше порога, отсчёт заново.
    let mut m = marked(&[("bingx", 1000)]);
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    m.apply(MarketEvent::Book(cake(at(t0, 900))));
    m.apply(MarketEvent::Book(cake(at(t0, 1900))));
    assert_eq!(since(&m, "10"), Some(t0));
    m.apply(MarketEvent::Book(cake(at(t0, 3000))));
    for q in ["10", "1000000"] {
        assert_eq!(since(&m, q), Some(at(t0, 3000)), "объём {q}");
    }
}

#[test]
fn no_gap_setting_for_venue_keeps_count() {
    // Порог задан только у другой площадки — у bingx разрыв не сбрасывает (котировка bingx без venue.book_stale_ms и
    // так отказывает, решение 238).
    let mut m = marked(&[("mexc", 1000)]);
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    m.apply(MarketEvent::Book(cake(at(t0, 5000))));
    assert_eq!(since(&m, "10"), Some(t0));
}

/// Направление DEX → биржа (сторона DEX — покупка).
fn dir() -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: bingx(),
    }
}

#[test]
fn quote_newer_than_core_book_keeps_hold() {
    // Ядро видело кадр t0; адаптер обновил свой стакан кадром t0 + 700 (ядру не дошёл — канал полон) и котирует по
    // нему. Стакан ядра старше кадра котировки — «стоит» не доказано → обычное удержание.
    let mut m = marked(&[]);
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    assert_eq!(since_q(&m, "10", t0), Some(t0));
    assert_eq!(since_q(&m, "10", at(t0, 700)), None);
    let p = Prices {
        buy: d("0.0022"),
        sell: d("0.0055"),
        fees_pct: d("0.2"),
        qty: d("10"),
        buy_at: at(t0, 1000),
        sell_at: at(t0, 700),
    };
    let still = since_q(&m, "10", p.sell_at).map(|s| (at(t0, 1000) - s).num_milliseconds() as u64);
    let par = Ok(Params {
        hold_ms: 500,
        hold_updates: 2,
        anomaly_pct: d("1000"),
        skip_on_pool: true,
        ripen: false,
    });
    let ctx = Ctx {
        dex_side: Some(Side::Buy),
        trigger: Trigger::Pool,
        cex_still_ms: still,
        ..Default::default()
    };
    let v = Signals::default().update_with(&dir(), Ok(p), &par, at(t0, 1000), &ctx);
    assert_eq!((v.state, v.skipped_hold), (State::Holding, false));
}

#[test]
fn flag_off_no_marks_then_count_from_first_frame() {
    let mut m = MarketState::default();
    assert!(!m.book_marks_on());
    let t0 = Utc::now();
    // Выключено: кадры применяются, отметок нет.
    m.apply(MarketEvent::Book(cake(t0)));
    m.apply(MarketEvent::Book(cake(at(t0, 100))));
    assert!(m.book(&bingx(), "CAKE").is_some());
    assert_eq!(since(&m, "10"), None);
    // Включили — до нового кадра отметок нет; первый кадр после включения (содержимое то же) — начало отсчёта.
    m.set_book_marks(true, HashMap::new());
    assert_eq!(since(&m, "10"), None);
    m.apply(MarketEvent::Book(cake(at(t0, 500))));
    assert_eq!(since(&m, "10"), Some(at(t0, 500)));
    m.apply(MarketEvent::Book(cake(at(t0, 900))));
    assert_eq!(since(&m, "10"), Some(at(t0, 500)));
    // Выключили — отметки очищены.
    m.set_book_marks(false, HashMap::new());
    assert_eq!(since(&m, "10"), None);
}
