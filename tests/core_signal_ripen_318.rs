//! Файл: tests/core_signal_ripen_318.rs
//! Что делает: решение 318 — таймер дозревания (signal.hold_timer). Чистая логика: фильтр шума (core/signal.rs —
//! срок ripen_at у «держится») и план таймера (core/signal_ripen.rs Plan — сроки, одна спящая задача на монету,
//! срабатывание). Время передаётся параметром (без sleep). Проверяет: (1) не задана — срока нет, прежние состояния;
//! (2) «держится» без тиков → пересчёт в above_since + min_hold_ms при живом спреде даёт кандидата; спред исчез →
//! кандидата нет; данные устарели (котировка отказала — «нет цены») → кандидата нет; обновлений мало — срок «сейчас»;
//! (3) план: не больше одной спящей задачи на монету, более ранний срок сменяет задачу, срабатывают только наступившие.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::signal, arb::core::signal_fast, arb::core::signal_ripen, arb::core::types. Моков нет.

use std::str::FromStr;

use arb::core::signal::{Params, Prices, Signals, State};
use arb::core::signal_fast::{Ctx, Trigger};
use arb::core::signal_ripen::Plan;
use arb::core::types::{Asset, Direction, Side, VenueId};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn dir(ticker: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("bingx".into()),
    }
}

/// Пороги живого бота 16.11: min_hold_ms 500; обновлений — параметром.
fn params(ripen: bool, updates: u64) -> Result<Params, String> {
    Ok(Params {
        hold_ms: 500,
        hold_updates: updates,
        anomaly_pct: d("1000"),
        skip_on_pool: false,
        ripen,
    })
}

fn prices(buy: &str, sell: &str) -> Result<Prices, String> {
    Ok(Prices {
        buy: d(buy),
        sell: d(sell),
        fees_pct: d("0.2"),
        qty: d("1000"),
        buy_at: Utc::now(),
        sell_at: Utc::now(),
    })
}

fn ctx(trigger: Trigger) -> Ctx {
    Ctx {
        dex_side: Some(Side::Buy),
        trigger,
        ..Default::default()
    }
}

fn at(t0: DateTime<Utc>, ms: i64) -> DateTime<Utc> {
    t0 + Duration::milliseconds(ms)
}

#[test]
fn off_no_deadline() {
    let mut s = Signals::default();
    let t0 = Utc::now();
    let v = s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &params(false, 2),
        t0,
        &ctx(Trigger::Pool),
    );
    assert_eq!((v.state, v.ripen_at), (State::Holding, None));
}

#[test]
fn timer_pass_with_live_spread_is_candidate() {
    let mut s = Signals::default();
    let t0 = Utc::now();
    let p = params(true, 2);
    let v = s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &p,
        t0,
        &ctx(Trigger::Pool),
    );
    assert_eq!(v.state, State::Holding);
    let due = v.ripen_at.expect("срок дозревания");
    assert_eq!(due, at(t0, 500));
    // Тиков нет — пересчёт ставит таймер в срок: повторная котировка, спред ещё есть.
    let v = s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &p,
        due,
        &ctx(Trigger::Timer),
    );
    assert_eq!(
        (v.state, v.updates, v.trigger),
        (State::Candidate, 2, Trigger::Timer)
    );
    assert_eq!(v.ripen_at, None);
}

#[test]
fn timer_pass_spread_gone_or_stale_no_candidate() {
    let t0 = Utc::now();
    let p = params(true, 2);
    // Спред исчез к сроку.
    let mut s = Signals::default();
    s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &p,
        t0,
        &ctx(Trigger::Pool),
    );
    let v = s.update_with(
        &dir("X"),
        prices("0.0055", "0.0055"),
        &p,
        at(t0, 500),
        &ctx(Trigger::Timer),
    );
    assert_eq!((v.state, v.ripen_at), (State::Below, None));
    // Данные устарели: котировка шлюза отказала — «нет цены», счётчики сброшены.
    let mut s = Signals::default();
    s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &p,
        t0,
        &ctx(Trigger::Pool),
    );
    let stale = Err("bingx: данные устарели".to_string());
    let v = s.update_with(&dir("X"), stale, &p, at(t0, 500), &ctx(Trigger::Timer));
    assert!(matches!(v.state, State::NoPrice(_)));
    assert_eq!(v.ripen_at, None);
}

#[test]
fn few_updates_deadline_now() {
    // min_hold_updates 3: в срок 500 мс — второе обновление, ещё «держится», следующий срок — сейчас.
    let mut s = Signals::default();
    let t0 = Utc::now();
    let p = params(true, 3);
    s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &p,
        t0,
        &ctx(Trigger::Pool),
    );
    let v = s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &p,
        at(t0, 500),
        &ctx(Trigger::Timer),
    );
    assert_eq!((v.state, v.ripen_at), (State::Holding, Some(at(t0, 500))));
    let v = s.update_with(
        &dir("X"),
        prices("0.0022", "0.0055"),
        &p,
        at(t0, 1000),
        &ctx(Trigger::Timer),
    );
    assert_eq!(v.state, State::Candidate);
}

#[test]
fn plan_one_task_per_coin() {
    let t0 = Utc::now();
    let (a, b) = (dir("X"), {
        let mut r = dir("X");
        std::mem::swap(&mut r.buy_on, &mut r.sell_on);
        r
    });
    let bingx = VenueId("bingx".into());
    let mut plan = Plan::default();
    plan.set(&a, Some(at(t0, 500)), &bingx);
    assert_eq!(plan.want_arm(&a.asset), Some(at(t0, 500)));
    // Второй срок позже — новая задача не нужна.
    plan.set(&b, Some(at(t0, 800)), &bingx);
    assert_eq!(plan.want_arm(&a.asset), None);
    // Более ранний срок — новая задача, прежняя при пробуждении ничего не делает.
    plan.set(&b, Some(at(t0, 300)), &bingx);
    assert_eq!(plan.want_arm(&a.asset), Some(at(t0, 300)));
    assert_eq!(plan.fire(&a.asset, at(t0, 500), at(t0, 500)), None);
    // Текущая задача: в 300 мс наступил только срок b.
    let v = plan.fire(&a.asset, at(t0, 300), at(t0, 300)).unwrap();
    assert_eq!(v, [bingx.clone()].into());
    assert!(plan.take_fired(&b));
    assert!(!plan.take_fired(&a));
    // Остался срок a — снова одна задача на 500 мс; снятый срок задачу не ставит.
    assert_eq!(plan.want_arm(&a.asset), Some(at(t0, 500)));
    plan.set(&a, None, &bingx);
    assert_eq!(
        plan.fire(&a.asset, at(t0, 500), at(t0, 500)),
        Some(Default::default())
    );
    assert_eq!(plan.want_arm(&a.asset), None);
}
