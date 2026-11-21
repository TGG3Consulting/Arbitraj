//! Файл: tests/core_signal_hold_317.rs
//! Что делает: решение 317 с доработкой 16.11 — сигнал от события пула без удержания (signal.dex_event_skip_hold).
//! Чистая логика, время — параметром (без sleep):
//! (1) фильтр шума (core/signal.rs, core/signal_fast.rs): выключено — прежние состояния (update_with = update);
//! включено — первое событие пула у тихой монеты после запуска и после «нет цены» при стоящем стакане → кандидат
//! сразу; стакан менялся < min_hold_ms назад, тик биржи, биржа ↔ биржа → обычное удержание;
//! (2) отметки изменения стакана в состоянии рынка ядра (core/book_still.rs через MarketState): тот же стакан
//! повторным кадром отметку не двигает; изменение уровня глубже объёма не двигает отметку для меньшего объёма (смена
//! объёма сравнения при неподвижных уровнях не мешает правилу); новый лучший уровень (мигающий) двигает отметку;
//! после «недостоверно» — отсчёт заново;
//! (3) шаг после пропуска удержания: кандидат не мигает (следующий пересчёт с held < min_hold_ms — кандидат), спред
//! ниже порога снимает пометку; правило выключили — прежний фильтр.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::signal, arb::core::signal_fast, arb::core::market_state, arb::core::types,
//! arb::venues::bingx::depth (только чтобы разобрать запись). Моков нет: стакан — реально записанный кадр BingX
//! CAKE-USDT@depth20 (tests/data/bingx/ws_spot_2026-10-23.jsonl); изменённые кадры — тот же кадр с одним изменённым
//! уровнем; цены сигнала — входные числа чистой функции.

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

/// Направление DEX → биржа (покупка на DEX — сторона DEX = Buy), как SHOPINX 16.11.
fn dir() -> Direction {
    Direction {
        asset: Asset {
            chain: None,
            ticker: "SHOPINX".into(),
            contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("bingx".into()),
    }
}

/// Пороги живого бота 16.11: min_hold_ms 500, min_hold_updates 2.
fn params(skip: bool) -> Result<Params, String> {
    Ok(Params {
        hold_ms: 500,
        hold_updates: 2,
        anomaly_pct: d("1000"),
        skip_on_pool: skip,
        ripen: false,
    })
}

/// Обвал цены в пуле: покупка на DEX 0.0022, продажа на бирже 0.0055 (спред выше комиссий 0,2 %).
fn crash() -> Result<Prices, String> {
    Ok(Prices {
        buy: d("0.0022"),
        sell: d("0.0055"),
        fees_pct: d("0.2"),
        qty: d("1000"),
        buy_at: Utc::now(),
        sell_at: Utc::now(),
    })
}

fn ctx(dex_side: Option<Side>, trigger: Trigger, still: Option<u64>) -> Ctx {
    Ctx {
        dex_side,
        trigger,
        cex_still_ms: still,
        ..Default::default()
    }
}

fn at(t0: DateTime<Utc>, ms: i64) -> DateTime<Utc> {
    t0 + Duration::milliseconds(ms)
}

#[test]
fn off_same_as_before() {
    // Выключено — те же состояния, что у прежнего фильтра (update без обстоятельств), даже при событии пула и стоящем
    // стакане.
    let (mut old, mut new) = (Signals::default(), Signals::default());
    let t0 = Utc::now();
    let p = params(false);
    let c = ctx(Some(Side::Buy), Trigger::Pool, Some(10_000));
    for ms in [0, 300, 600] {
        let a = old.update(&dir(), crash(), &p, at(t0, ms)).state;
        let b = new.update_with(&dir(), crash(), &p, at(t0, ms), &c);
        assert_eq!((a, false), (b.state, b.skipped_hold));
    }
}

#[test]
fn first_pool_event_after_start_is_candidate() {
    // Первый пересчёт после запуска (памяти фильтра нет), стакан биржи стоит 600 мс ≥ 500.
    let v = Signals::default().update_with(
        &dir(),
        crash(),
        &params(true),
        Utc::now(),
        &ctx(Some(Side::Buy), Trigger::Pool, Some(600)),
    );
    assert_eq!((v.state, v.skipped_hold), (State::Candidate, true));
    assert_eq!(v.cex_still_ms, Some(600));
}

#[test]
fn first_pool_event_after_no_price_is_candidate() {
    // Прошлый пересчёт — «нет цены» (правила свежести), стакан с тех пор не менялся.
    let mut s = Signals::default();
    let t0 = Utc::now();
    let p = params(true);
    let c = ctx(Some(Side::Buy), Trigger::Pool, Some(700));
    s.update_with(&dir(), Err("нет котировки".into()), &p, t0, &c);
    let v = s.update_with(&dir(), crash(), &p, at(t0, 100), &c);
    assert_eq!((v.state, v.skipped_hold), (State::Candidate, true));
}

#[test]
fn recent_book_change_keeps_hold() {
    let v = Signals::default().update_with(
        &dir(),
        crash(),
        &params(true),
        Utc::now(),
        &ctx(Some(Side::Buy), Trigger::Pool, Some(200)),
    );
    assert_eq!((v.state, v.skipped_hold), (State::Holding, false));
}

#[test]
fn book_tick_or_no_dex_side_does_not_skip() {
    let p = params(true);
    for c in [
        ctx(Some(Side::Buy), Trigger::Book, Some(10_000)),
        ctx(Some(Side::Buy), Trigger::Timer, Some(10_000)),
        ctx(None, Trigger::Pool, Some(10_000)),
        ctx(Some(Side::Buy), Trigger::Pool, None),
    ] {
        let v = Signals::default().update_with(&dir(), crash(), &p, Utc::now(), &c);
        assert_eq!((v.state, v.skipped_hold), (State::Holding, false), "{c:?}");
    }
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
                    venue: VenueId("bingx".into()),
                    asset: Asset {
                        chain: None,
                        ticker: "CAKE".into(),
                        contract: None,
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

/// Состояние рынка с включёнными отметками стакана (решение 317, ревью 16.11: признак book_marks_on.rs — при
/// signal.dex_event_skip_hold или log.signal_hold).
fn marked() -> MarketState {
    let mut m = MarketState::default();
    m.set_book_marks(true, HashMap::new());
    m
}

/// С какого момента стоят покупатели (продажа на бирже) на объёме qty; котировка — по кадру не новее стакана ядра
/// (страховка решения 317 не мешает — её проверяет quote_newer_than_core_book_keeps_hold).
fn since(m: &MarketState, qty: &str) -> Option<DateTime<Utc>> {
    m.book_still_since(
        &VenueId("bingx".into()),
        "CAKE",
        Side::Sell,
        d(qty),
        DateTime::<Utc>::MIN_UTC,
    )
}

#[test]
fn same_book_frame_does_not_move_mark() {
    let mut m = marked();
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    // Снимок с тем же содержимым через 400 и 900 мс (снимки потоком) — отметка остаётся t0.
    m.apply(MarketEvent::Book(cake(at(t0, 400))));
    m.apply(MarketEvent::Book(cake(at(t0, 900))));
    for q in ["10", "1000", "6000", "1000000"] {
        assert_eq!(since(&m, q), Some(t0), "объём {q}");
    }
}

#[test]
fn deep_level_change_moves_mark_only_for_volume_reaching_it() {
    // Покупатели записи: 12.903, 1089.137, 849.992, 1054.736, 2191.916, 1229.615, … — объём 3000 покрывают первые 4
    // уровня, 6000 — 6. Меняется количество 5-го уровня (индекс 4).
    let mut m = marked();
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    let mut b = cake(at(t0, 400));
    b.bids[4].qty += d("1");
    m.apply(MarketEvent::Book(b));
    // Смена объёма сравнения при неподвижных уровнях (10 → 1000 → 3000) — отметка та же.
    for q in ["10", "1000", "3000"] {
        assert_eq!(since(&m, q), Some(t0), "объём {q}");
    }
    assert_eq!(since(&m, "6000"), Some(at(t0, 400)));
}

#[test]
fn blinking_best_level_moves_mark() {
    // Появился новый лучший уровень покупателей (призрачный) — отметка с этого кадра для любого объёма: через 100 мс
    // стакан «стоит» 100 мс < signal.min_hold_ms → обычное удержание.
    let mut m = marked();
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    let mut b = cake(at(t0, 1000));
    b.bids.insert(
        0,
        PriceLevel {
            price: d("2.300"),
            qty: d("5000"),
        },
    );
    m.apply(MarketEvent::Book(b));
    assert_eq!(since(&m, "10"), Some(at(t0, 1000)));
    // Уровень исчез следующим кадром — снова изменение, отметка двигается.
    m.apply(MarketEvent::Book(cake(at(t0, 1200))));
    assert_eq!(since(&m, "10"), Some(at(t0, 1200)));
    let still = (at(t0, 1300) - since(&m, "10").unwrap()).num_milliseconds() as u64;
    let v = Signals::default().update_with(
        &dir(),
        crash(),
        &params(true),
        at(t0, 1300),
        &ctx(Some(Side::Buy), Trigger::Pool, Some(still)),
    );
    assert_eq!((v.state, v.skipped_hold), (State::Holding, false));
}

#[test]
fn invalidated_restarts_mark() {
    let mut m = marked();
    let t0 = Utc::now();
    m.apply(MarketEvent::Book(cake(t0)));
    m.apply(MarketEvent::Invalidated {
        venue: VenueId("bingx".into()),
        ticker: "CAKE".into(),
        address: None,
    });
    assert_eq!(since(&m, "10"), None);
    m.apply(MarketEvent::Book(cake(at(t0, 2000))));
    assert_eq!(since(&m, "10"), Some(at(t0, 2000)));
}

#[test]
fn step_after_skip_keeps_candidate() {
    // Пропуск удержания в t0; следующий пересчёт через 100 мс (тик биржи, held 100 < 500, обновлений 2) — кандидат
    // остаётся (эпизод и окно не закрываются); спред ушёл ниже порога — пометка снята, новое событие пула при
    // недавно изменённом стакане — обычное удержание.
    let mut s = Signals::default();
    let t0 = Utc::now();
    let p = params(true);
    let pool = ctx(Some(Side::Buy), Trigger::Pool, Some(600));
    let book = ctx(Some(Side::Buy), Trigger::Book, Some(50));
    let v = s.update_with(&dir(), crash(), &p, t0, &pool);
    assert_eq!((v.state, v.skipped_hold), (State::Candidate, true));
    let v = s.update_with(&dir(), crash(), &p, at(t0, 100), &book);
    assert_eq!((v.state, v.updates), (State::Candidate, 2));
    let below = Ok(Prices {
        buy: d("0.0055"),
        sell: d("0.0055"),
        fees_pct: d("0.2"),
        qty: d("1000"),
        buy_at: Utc::now(),
        sell_at: Utc::now(),
    });
    assert_eq!(
        s.update_with(&dir(), below, &p, at(t0, 200), &book).state,
        State::Below
    );
    let recent = ctx(Some(Side::Buy), Trigger::Pool, Some(100));
    let v = s.update_with(&dir(), crash(), &p, at(t0, 300), &recent);
    assert_eq!((v.state, v.skipped_hold), (State::Holding, false));
}

#[test]
fn step_after_skip_rule_off_is_old_filter() {
    // Пропуск был, правило выключили — следующий пересчёт по прежнему фильтру («держится»).
    let mut s = Signals::default();
    let t0 = Utc::now();
    s.update_with(
        &dir(),
        crash(),
        &params(true),
        t0,
        &ctx(Some(Side::Buy), Trigger::Pool, Some(600)),
    );
    let v = s.update_with(
        &dir(),
        crash(),
        &params(false),
        at(t0, 100),
        &ctx(Some(Side::Buy), Trigger::Book, Some(700)),
    );
    assert_eq!(v.state, State::Holding);
}
