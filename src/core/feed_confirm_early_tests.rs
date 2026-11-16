//! Файл: core/feed_confirm_early_tests.rs
//! Что проверяет: t163 (решение 366, для всех бирж) — сторож спрашивает биржу о тихой паре ДО того, как котировка
//! признает стакан устаревшим (тишина больше venue.book_stale_ms − venue.pair_resub_interval_ms), ответ на такой ранний
//! запрос не выбрасывается (раньше — «кадр моложе book_stale_ms × pair_dead_mult» → ответ терялся, пара между
//! book_stale_ms и book_stale_ms × pair_dead_mult стояла в отказе «кадра стакана пары нет»), подтверждение покрывает
//! момент, когда кадр стал бы старше book_stale_ms; повторный запрос — не раньше venue.quiet_confirm_min_ms, после нового
//! кадра — сразу; кадр после отправки — ответ не нужен; без подтверждения (журнал, переподписка 202) — как раньше.
//! Вход — снимок свежести формата стыка (gateway/feed_health.rs), числа — советы docs/FIRST_RUN.md, как в
//! feed_confirm_tests.rs. Без моков сети.
//! Кто вызывает: cargo test.

use std::collections::HashMap;

use super::feed_confirm::{QuietCfg, apply};
use super::feed_watch_logic::{Action, Cfg, State, decide};
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
        dead_pct: 30,
        reconnect_min_ms: 120_000,
        reconnect_max_per_hour: 3,
        quiet: QuietCfg::Confirm {
            min_ms: 30_000,
            max_per_s: 5,
        },
    }
}

/// Снимок: одно соединение «m1», начато в 0, живо в now; пара GMT — последний кадр и номер книги.
fn health(now: i64, last: i64, id: i64) -> FeedHealth {
    FeedHealth {
        scheduled: false,
        per_pair: true,
        confirm: true,
        conns: vec![ConnHealth {
            conn: "m1".into(),
            alive_ms: Some(now),
            started_ms: Some(0),
            pairs: vec![PairHealth {
                ticker: "GMT".into(),
                last_ms: Some(last),
                since_ms: 0,
                last_id: Some(id),
                last_changed_ms: None,
            }],
        }],
    }
}

fn by_id(id: i64) -> Result<BookVersion, String> {
    Ok(BookVersion {
        id: Some(id),
        ..Default::default()
    })
}

fn check() -> Action {
    Action::Check {
        conn: "m1".into(),
        ticker: "GMT".into(),
    }
}

#[test]
fn quiet_pair_confirmed_before_quote_calls_it_stale() {
    let (c, mut st, none) = (cfg(), State::default(), HashMap::new());
    // Тишина 1,5 с — до порога котировки (5 с) больше прохода сторожа (3 с): не спрашивать.
    assert!(decide(&health(40_000, 38_500, 100), 40_000, &c, &none, &mut st).is_empty());
    // Тишина 2,1 с: до следующего прохода кадр станет старше 5 с — спросить сейчас (раньше — только после 15 с).
    assert_eq!(
        decide(&health(40_600, 38_500, 100), 40_600, &c, &none, &mut st),
        vec![check()]
    );
    // Книга на бирже того же номера — подтверждена (раньше ответ при кадре моложе 15 с выбрасывался).
    let a = apply(
        &health(40_700, 38_500, 100),
        "m1",
        "GMT",
        by_id(100),
        40_600,
        40_700,
        &c,
        &mut st,
    );
    let until = 40_600 + 30_000 + 3_000;
    assert_eq!(
        a,
        vec![Action::Confirmed {
            conn: "m1".into(),
            ticker: "GMT".into(),
            until_ms: until,
            log: true,
        }]
    );
    // Кадр стал бы старше 5 с в 43 500 — подтверждение уже действует и дольше.
    assert!(until > 38_500 + c.book_stale_ms);
    // До venue.quiet_confirm_min_ms — без новых запросов.
    assert!(decide(&health(43_600, 38_500, 100), 43_600, &c, &none, &mut st).is_empty());
    assert!(decide(&health(70_000, 38_500, 100), 70_000, &c, &none, &mut st).is_empty());
    assert_eq!(
        decide(&health(70_600, 38_500, 100), 70_600, &c, &none, &mut st),
        vec![check()]
    );
}

#[test]
fn new_frame_resets_check_and_frame_after_send_skips_answer() {
    let (c, mut st, none) = (cfg(), State::default(), HashMap::new());
    assert_eq!(
        decide(&health(40_600, 38_500, 100), 40_600, &c, &none, &mut st),
        vec![check()]
    );
    // Кадр пришёл после отправки запроса (40 650 > 40 600) — стакан в памяти новее ответа, ответ не нужен.
    let a = apply(
        &health(40_700, 40_650, 101),
        "m1",
        "GMT",
        by_id(100),
        40_600,
        40_700,
        &c,
        &mut st,
    );
    assert!(a.is_empty(), "{a:?}");
    // Новый кадр после проверки — срок проверки сброшен: замолчала на 2,1 с — снова спросить, не ждать 30 с.
    assert!(decide(&health(50_000, 50_000, 102), 50_000, &c, &none, &mut st).is_empty());
    assert_eq!(
        decide(&health(52_100, 50_000, 102), 52_100, &c, &none, &mut st),
        vec![check()]
    );
}

#[test]
fn without_confirm_nothing_changes() {
    let mut c = cfg();
    c.quiet = QuietCfg::Off("не задана venue.quiet_confirm_min_ms".into());
    let (mut st, none) = (State::default(), HashMap::new());
    // Тишина 6 с (< 15 с порога мёртвой пары): ни проверки, ни журнала, ни переподписки — как до t163.
    assert!(decide(&health(46_000, 40_000, 100), 46_000, &c, &none, &mut st).is_empty());
}
