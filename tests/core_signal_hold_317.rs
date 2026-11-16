//! Файл: tests/core_signal_hold_317.rs
//! Что делает: решение 317 — сигнал от события пула без удержания (signal.dex_event_skip_hold). Чистая логика фильтра
//! шума (core/signal.rs, core/signal_fast.rs): последовательности пересчётов со временем, переданным параметром (без
//! sleep). Проверяет: (1) выключено — прежнее поведение на тех же последовательностях (update_with = update);
//! (2) включено: событие пула + цена биржи стоит ≥ min_hold_ms → кандидат сразу; цена биржи менялась недавно, тик
//! биржи вместо события пула, биржа ↔ биржа, пропуск цены между пересчётами → обычное удержание.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::signal, arb::core::signal_fast, arb::core::types. Моков нет: цены — входные числа
//! чистой функции сигнала (площадки не участвуют).

use std::str::FromStr;

use arb::core::signal::{Params, Prices, Signals, State};
use arb::core::signal_fast::{Ctx, Trigger};
use arb::core::types::{Asset, Direction, Side, VenueId};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Направление DEX → биржа (покупка на DEX — сторона DEX = Buy), как SHOPINX 16.11.
fn dir() -> Direction {
    Direction {
        asset: Asset {
            ticker: "SHOPINX".into(),
            bsc_contract: None,
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
    })
}

fn prices(buy: &str, sell: &str) -> Result<Prices, String> {
    Ok(Prices {
        buy: d(buy),
        sell: d(sell),
        fees_pct: d("0.2"),
        qty: d("1000"),
    })
}

fn ctx(dex_side: Option<Side>, trigger: Trigger) -> Ctx {
    Ctx { dex_side, trigger }
}

fn at(t0: DateTime<Utc>, ms: i64) -> DateTime<Utc> {
    t0 + Duration::milliseconds(ms)
}

/// Шаг последовательности: мс от начала, цена покупки на DEX, цена продажи на бирже, что запустило.
type Step = (i64, &'static str, &'static str, Trigger);

/// Обвал цены в пуле: цена биржи 0.0055 стоит с 0 мс; в 2000 мс событие пула — цена DEX 0.0022.
const CRASH: &[Step] = &[
    (0, "0.0055", "0.0055", Trigger::Book),
    (1000, "0.0055", "0.0055", Trigger::Pool),
    (2000, "0.0022", "0.0055", Trigger::Pool),
    (2300, "0.0022", "0.0055", Trigger::Pool),
    (2600, "0.0022", "0.0055", Trigger::Book),
];

fn run(steps: &[Step], skip: bool, side: Option<Side>) -> Vec<(State, bool)> {
    let mut s = Signals::default();
    let t0 = Utc::now();
    let p = params(skip);
    steps
        .iter()
        .map(|(ms, buy, sell, tr)| {
            let v = s.update_with(&dir(), prices(buy, sell), &p, at(t0, *ms), &ctx(side, *tr));
            (v.state, v.skipped_hold)
        })
        .collect()
}

#[test]
fn off_same_as_before() {
    // Прежний фильтр (update без обстоятельств) и update_with при выключенном правиле — одни и те же состояния.
    let mut old = Signals::default();
    let t0 = Utc::now();
    let p = params(false);
    let before: Vec<_> = CRASH
        .iter()
        .map(|(ms, b, s, _)| {
            (
                old.update(&dir(), prices(b, s), &p, at(t0, *ms)).state,
                false,
            )
        })
        .collect();
    assert_eq!(run(CRASH, false, Some(Side::Buy)), before);
    assert_eq!(
        before.iter().map(|x| x.0.clone()).collect::<Vec<_>>(),
        vec![
            State::Below,
            State::Below,
            State::Holding,
            State::Holding,
            State::Candidate
        ]
    );
}

#[test]
fn pool_event_with_still_book_is_candidate_at_once() {
    let r = run(CRASH, true, Some(Side::Buy));
    assert_eq!(
        r[2],
        (State::Candidate, true),
        "кандидат с первого обновления"
    );
}

#[test]
fn recent_book_change_keeps_hold() {
    // Цена биржи сменилась в 1800 мс (спред 0,18 % — ниже комиссий 0,2 %) — к событию пула в 2000 мс стоит
    // 200 мс < 500: обычное удержание; в 2600 мс — кандидат по обычному правилу (600 мс, 2 обновления).
    let steps: &[Step] = &[
        (0, "0.0055", "0.0055", Trigger::Book),
        (1800, "0.0055", "0.00551", Trigger::Book),
        (2000, "0.0022", "0.00551", Trigger::Pool),
        (2600, "0.0022", "0.00551", Trigger::Pool),
    ];
    let r = run(steps, true, Some(Side::Buy));
    assert_eq!(r[2], (State::Holding, false));
    assert_eq!(r[3], (State::Candidate, false));
}

#[test]
fn book_tick_does_not_skip() {
    // Та же картина, но пересчёт запущен тиком биржи — правило не действует.
    let steps: Vec<Step> = CRASH
        .iter()
        .map(|(a, b, c, _)| (*a, *b, *c, Trigger::Book))
        .collect();
    assert_eq!(
        run(&steps, true, Some(Side::Buy))[2],
        (State::Holding, false)
    );
}

#[test]
fn cex_cex_and_dex_dex_unchanged() {
    // Нет стороны DEX (биржа ↔ биржа, DEX ↔ DEX) — то же, что выключено.
    assert_eq!(run(CRASH, true, None), run(CRASH, false, None));
}

#[test]
fn no_price_between_resets_still_time() {
    // Пропала цена в 1500 мс — память сторон сброшена: в 2000 мс цена биржи «стоит» 0 мс.
    let mut s = Signals::default();
    let t0 = Utc::now();
    let p = params(true);
    let c = ctx(Some(Side::Buy), Trigger::Pool);
    s.update_with(&dir(), prices("0.0055", "0.0055"), &p, t0, &c);
    s.update_with(&dir(), Err("нет котировки".into()), &p, at(t0, 1500), &c);
    s.update_with(&dir(), prices("0.0055", "0.0055"), &p, at(t0, 1900), &c);
    let v = s.update_with(&dir(), prices("0.0022", "0.0055"), &p, at(t0, 2000), &c);
    assert_eq!((v.state, v.skipped_hold), (State::Holding, false));
    assert_eq!(v.cex_still_ms, Some(100));
}
