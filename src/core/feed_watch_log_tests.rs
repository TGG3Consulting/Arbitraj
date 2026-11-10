//! Файл: core/feed_watch_log_tests.rs
//! Что проверяет: дополнение 10.11 к решениям 202/213 — журнал видимости сторожа (feed_watch_log.rs): без
//! venue.feed_watch_log_ms строки по парам как раньше (при смене состояния), сводки нет, строка «работает» — одна при
//! включении; с настройкой — строка вида решения по паре не чаще раза в N мс (и повторы), сводка не чаще раза в N мс со
//! счётом пар по решениям сторожа. Вход — снимок свежести формата стыка (gateway/feed_health.rs) и состояние после
//! настоящих проходов decide / apply; моков нет.
//! Кто вызывает: cargo test.

use std::collections::HashMap;

use super::feed_confirm::{QuietCfg, apply};
use super::feed_watch_log::LogGate;
use super::feed_watch_logic::{Cfg, State, decide};
use crate::gateway::feed_health::{BookVersion, ConnHealth, FeedHealth, PairHealth};

fn cfg() -> Cfg {
    Cfg {
        book_stale_ms: 5_000,
        dead_mult: 3,
        live_ms: 40_000,
        warmup_ms: 30_000,
        attempts: 3,
        interval_ms: 3_000,
        pause_ms: 60_000,
        dead_pct: 90,
        reconnect_min_ms: 120_000,
        reconnect_max_per_hour: 3,
        quiet: QuietCfg::Confirm {
            min_ms: 30_000,
            max_per_s: 5,
        },
    }
}

fn health(now: i64) -> FeedHealth {
    let p = |t: &str, last: i64| PairHealth {
        ticker: t.into(),
        last_ms: Some(last),
        since_ms: 0,
        last_id: Some(100),
        last_changed_ms: None,
    };
    FeedHealth {
        scheduled: false,
        per_pair: true,
        confirm: true,
        conns: vec![ConnHealth {
            conn: "m1".into(),
            alive_ms: Some(now),
            started_ms: Some(0),
            pairs: vec![p("QUIET", 10_000), p("LOST", 10_000), p("LIVE", now)],
        }],
    }
}

#[test]
fn pair_lines_without_setting_as_before() {
    let mut lg = LogGate::default();
    assert!(lg.pair("m1", "A", "quiet", true, 1_000));
    assert!(
        !lg.pair("m1", "A", "quiet", false, 2_000),
        "без смены состояния — молча"
    );
    assert!(
        lg.summary(&health(1_000), 1_000, &cfg(), &State::default())
            .is_none()
    );
    let h = health(1_000);
    let first = lg
        .announce(&h, &cfg())
        .expect("строка «работает» при включении");
    assert!(
        first.contains("пар 3") && first.contains("не задана"),
        "{first}"
    );
    assert!(lg.announce(&h, &cfg()).is_none(), "один раз");
    lg.off();
    assert!(
        lg.announce(&h, &cfg()).is_some(),
        "после выключения и включения — снова"
    );
}

#[test]
fn pair_lines_and_summary_with_setting() {
    let mut lg = LogGate::default();
    lg.every = Some(60_000);
    assert!(
        lg.pair("m1", "A", "quiet", false, 1_000),
        "повтор тоже виден"
    );
    assert!(
        !lg.pair("m1", "A", "quiet", true, 30_000),
        "чаще раза в 60 с — нет"
    );
    assert!(
        lg.pair("m1", "A", "lost", true, 30_000),
        "другой вид решения — своя частота"
    );
    assert!(
        lg.pair("m1", "B", "quiet", false, 30_000),
        "другая пара — своя частота"
    );
    assert!(lg.pair("m1", "A", "quiet", false, 61_000));

    // Состояние после настоящих проходов: QUIET подтверждена, LOST — кадры потеряны (круг), LIVE — свежая.
    let (c, mut st, none) = (cfg(), State::default(), HashMap::new());
    let checks = decide(&health(40_000), 40_000, &c, &none, &mut st);
    assert_eq!(checks.len(), 2, "{checks:?}");
    let v = |id| {
        Ok(BookVersion {
            id: Some(id),
            ..Default::default()
        })
    };
    let h = health(40_100);
    for a in apply(&h, "m1", "QUIET", v(100), 40_000, 40_100, &c, &mut st)
        .iter()
        .chain(&apply(
            &h,
            "m1",
            "LOST",
            v(105),
            40_000,
            40_100,
            &c,
            &mut st,
        ))
    {
        lg.count(a);
    }
    let s = lg.summary(&h, 40_100, &c, &st).expect("сводка");
    for part in [
        "соединений 1 (живых 1)",
        "свежих пар 1",
        "молчат 2",
        "тихие подтверждены 1",
        "кадры потеряны 1",
        "в кругах переподписки (попытки или пауза) 1",
        "переподписок 1",
    ] {
        assert!(s.contains(part), "нет «{part}»: {s}");
    }
    assert!(
        lg.summary(&h, 50_000, &c, &st).is_none(),
        "не чаще раза в 60 с"
    );
    let s = lg.summary(&h, 100_100, &c, &st).expect("сводка через 60 с");
    assert!(s.contains("переподписок 0"), "счётчики обнулены: {s}");
}
