//! Файл: core/feed_confirm_tests.rs
//! Что проверяет: решение 213 — подтверждение тихой пары по REST в чистой логике сторожа (feed_watch_logic.rs decide,
//! feed_confirm.rs apply / verdict): тихая подтверждённая (не переподписывается, срок свежести, повтор не раньше
//! venue.quiet_confirm_min_ms, журнал раз на смену состояния), книга новее → кадры потеряны → круг переподписки и
//! после паузы снова проверка, «не менялась с переподписки», ошибка REST (журнал раз, вслепую не переподписывать;
//! при кадрах по расписанию — переподписка), пара без кадров с подписки, кадр пришёл во время запроса, ведро
//! запросов площадки, площадка без подтверждения — только журнал. Без моков сети: вход — снимок свежести формата
//! стыка (gateway/feed_health.rs), числа — советы docs/FIRST_RUN.md. Сравнение на записанных ответах Gate —
//! tests/gate_real_data.rs.
//! Кто вызывает: cargo test.

use std::collections::HashMap;

use super::feed_confirm::{QuietCfg, Verdict, apply, verdict};
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

/// Пара: тикер, последний кадр, номер книги последнего кадра, (пере)подписка.
fn pair(t: &str, last: Option<i64>, id: Option<i64>, since: i64) -> PairHealth {
    PairHealth {
        ticker: t.into(),
        last_ms: last,
        since_ms: since,
        last_id: id,
        last_changed_ms: None,
    }
}

/// Снимок: одно соединение «m1», начато в 0, живо в now.
fn health(now: i64, scheduled: bool, confirm: bool, pairs: Vec<PairHealth>) -> FeedHealth {
    FeedHealth {
        scheduled,
        per_pair: true,
        confirm,
        conns: vec![ConnHealth {
            conn: "m1".into(),
            alive_ms: Some(now),
            started_ms: Some(0),
            pairs,
        }],
    }
}

fn ver(id: i64, changed: i64, server: i64) -> BookVersion {
    BookVersion {
        id: Some(id),
        changed_ms: Some(changed),
        server_ms: Some(server),
    }
}

fn check(t: &str) -> Action {
    Action::Check {
        conn: "m1".into(),
        ticker: t.into(),
    }
}

#[test]
fn quiet_confirmed_not_resubscribed() {
    let (c, mut st, none) = (cfg(), State::default(), HashMap::new());
    let h = |n| {
        health(
            n,
            false,
            true,
            vec![pair("AIL", Some(10_000), Some(100), 0)],
        )
    };
    // Молчит 30 с (> 15 с) после прогрева — спросить биржу, а не переподписывать.
    assert_eq!(
        decide(&h(40_000), 40_000, &c, &none, &mut st),
        vec![check("AIL")]
    );
    // Книга на бирже того же номера — подтверждена: свежая до запроса + 30 с + интервал прохода 3 с.
    let r = Ok(ver(100, 9_000, 40_050));
    let a = apply(&h(40_100), "m1", "AIL", r, 40_000, 40_100, &c, &mut st);
    let ok = |until_ms, log| Action::Confirmed {
        conn: "m1".into(),
        ticker: "AIL".into(),
        until_ms,
        log,
    };
    assert_eq!(a, vec![ok(73_000, true)]);
    // До venue.quiet_confirm_min_ms — ни проверки, ни переподписки.
    assert!(decide(&h(43_000), 43_000, &c, &none, &mut st).is_empty());
    assert!(decide(&h(69_000), 69_000, &c, &none, &mut st).is_empty());
    assert_eq!(
        decide(&h(70_000), 70_000, &c, &none, &mut st),
        vec![check("AIL")]
    );
    // Снова тихая — срок продлён, в журнал второй раз не пишется.
    let r = Ok(ver(100, 9_000, 70_050));
    let a = apply(&h(70_100), "m1", "AIL", r, 70_000, 70_100, &c, &mut st);
    assert_eq!(a, vec![ok(103_000, false)]);
}

#[test]
fn newer_book_lost_frames_resub_then_check_again() {
    let (c, mut st, none) = (cfg(), State::default(), HashMap::new());
    let h = |n| {
        health(
            n,
            false,
            true,
            vec![pair("AIL", Some(10_000), Some(100), 0)],
        )
    };
    assert_eq!(
        decide(&h(40_000), 40_000, &c, &none, &mut st),
        vec![check("AIL")]
    );
    // На бирже номер 105 (новее 100), книга менялась 1 с назад — после подписки: кадры потеряны.
    let a = apply(
        &h(40_100),
        "m1",
        "AIL",
        Ok(ver(105, 39_000, 40_050)),
        40_000,
        40_100,
        &c,
        &mut st,
    );
    assert!(
        matches!(&a[..], [Action::Lost { why, .. }, Action::Resub { attempt: 1, round: 1, .. }] if why.contains("105")),
        "{a:?}"
    );
    // Круг 202 без изменений: попытки 2 и 3 по интервалу, затем пауза.
    let resub = |a: &[Action]| {
        a.iter()
            .filter(|x| matches!(x, Action::Resub { .. }))
            .count()
    };
    assert_eq!(resub(&decide(&h(43_100), 43_100, &c, &none, &mut st)), 1);
    assert_eq!(resub(&decide(&h(46_100), 46_100, &c, &none, &mut st)), 1);
    assert!(
        decide(&h(49_100), 49_100, &c, &none, &mut st).is_empty(),
        "конец круга — пауза"
    );
    assert!(
        decide(&h(80_000), 80_000, &c, &none, &mut st).is_empty(),
        "в паузе ни проверки, ни попыток"
    );
    // После паузы — снова проверка у биржи, а не новый круг вслепую.
    assert_eq!(
        decide(&h(109_100), 109_100, &c, &none, &mut st),
        vec![check("AIL")]
    );
}

#[test]
fn unchanged_since_resub_and_no_frames_since_sub() {
    let c = cfg();
    // Кадр книги 100 был, пара переподписана в 35 с; на бирже 105, но книга менялась 50 с назад по часам биржи —
    // до переподписки (с неё 35,1 с по часам бота): кадр Gate пришлёт при изменении, переподписка не поможет.
    let p = pair("AIL", Some(10_000), Some(100), 35_000);
    assert_eq!(
        verdict(&p, &ver(105, 20_000, 70_050), 70_100),
        Verdict::Unchanged
    );
    // Кадров с подписки нет (Gate: AIL, KILO, PELL — update старше 1,5–6,5 ч): книга старше подписки — не менялась.
    let p = pair("KILO", None, None, 0);
    let five_h = 5 * 3_600_000;
    assert_eq!(
        verdict(&p, &ver(7, 1, 1 + five_h), 40_100),
        Verdict::Unchanged
    );
    // Та же пара, но книга менялась 1 с назад, а кадра нет — кадры потеряны.
    assert!(matches!(
        verdict(&p, &ver(7, 39_000, 40_000), 40_100),
        Verdict::Lost(_)
    ));
    // Время ответа не дано — возраст книги неизвестен: считать потерянными (переподписка по 202).
    let v = BookVersion {
        server_ms: None,
        ..ver(7, 1, 1)
    };
    assert!(matches!(verdict(&p, &v, 40_100), Verdict::Lost(_)));
    // Кадр есть, а сравнимой версии нет — Unknown.
    let p = pair("XCV", Some(10_000), None, 0);
    assert!(matches!(
        verdict(&p, &ver(7, 1, 2), 40_100),
        Verdict::Unknown(_)
    ));
    // Время изменения по часам биржи у кадра и ответа (биржи без номера).
    let mut p = pair("U", Some(10_000), None, 0);
    p.last_changed_ms = Some(500);
    let v = BookVersion {
        id: None,
        ..ver(0, 500, 9_000)
    };
    assert_eq!(verdict(&p, &v, 40_100), Verdict::Quiet);
    // В apply: «не менялась» — журнал раз, без переподписки.
    let (mut st, none) = (State::default(), HashMap::new());
    let h = |n| {
        health(
            n,
            false,
            true,
            vec![pair("AIL", Some(10_000), Some(100), 35_000)],
        )
    };
    assert_eq!(
        decide(&h(70_000), 70_000, &c, &none, &mut st),
        vec![check("AIL")]
    );
    let r = |server| Ok(ver(105, 20_000, server));
    let a = apply(
        &h(70_100),
        "m1",
        "AIL",
        r(70_050),
        70_000,
        70_100,
        &c,
        &mut st,
    );
    assert!(matches!(&a[..], [Action::Unchanged { .. }]), "{a:?}");
    let a = apply(
        &h(100_100),
        "m1",
        "AIL",
        r(100_050),
        100_000,
        100_100,
        &c,
        &mut st,
    );
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn rest_error_logged_once_no_blind_resub() {
    let (c, mut st) = (cfg(), State::default());
    let p = || vec![pair("AIL", Some(10_000), Some(100), 0)];
    let err = || Err("HTTP 503".to_string());
    let h = health(40_100, false, true, p());
    let a = apply(&h, "m1", "AIL", err(), 40_000, 40_100, &c, &mut st);
    assert!(
        matches!(&a[..], [Action::CheckFailed { why, .. }] if why.contains("503")),
        "{a:?}"
    );
    assert!(
        apply(&h, "m1", "AIL", err(), 70_000, 70_100, &c, &mut st).is_empty(),
        "та же причина — молча"
    );
    // Кадры по расписанию по документации — без подтверждения тишина = мёртвая подписка (решение 202).
    let mut st = State::default();
    let h = health(40_100, true, true, p());
    let a = apply(&h, "m1", "AIL", err(), 40_000, 40_100, &c, &mut st);
    assert!(
        matches!(
            &a[..],
            [Action::CheckFailed { .. }, Action::Resub { attempt: 1, .. }]
        ),
        "{a:?}"
    );
}

#[test]
fn frame_during_request_and_bucket() {
    let c = cfg();
    let mut st = State::default();
    // Пока шёл запрос, пришёл кадр — ответ не нужен.
    let h = health(
        40_100,
        false,
        true,
        vec![pair("AIL", Some(40_090), Some(101), 0)],
    );
    assert!(
        apply(
            &h,
            "m1",
            "AIL",
            Ok(ver(101, 1, 2)),
            40_000,
            40_100,
            &c,
            &mut st
        )
        .is_empty()
    );
    // Ведро: 1 запрос/с на площадку — из трёх тихих пар одна за проход, следующая через секунду.
    let mut c = cfg();
    c.quiet = QuietCfg::Confirm {
        min_ms: 30_000,
        max_per_s: 1,
    };
    let (mut st, none) = (State::default(), HashMap::new());
    let ps = || {
        ["A", "B", "C"]
            .map(|t| pair(t, Some(10_000), Some(1), 0))
            .to_vec()
    };
    let n = |a: Vec<Action>| {
        a.iter()
            .filter(|x| matches!(x, Action::Check { .. }))
            .count()
    };
    assert_eq!(
        n(decide(
            &health(40_000, false, true, ps()),
            40_000,
            &c,
            &none,
            &mut st
        )),
        1
    );
    assert_eq!(
        n(decide(
            &health(40_500, false, true, ps()),
            40_500,
            &c,
            &none,
            &mut st
        )),
        0
    );
    assert_eq!(
        n(decide(
            &health(41_000, false, true, ps()),
            41_000,
            &c,
            &none,
            &mut st
        )),
        1
    );
}

#[test]
fn no_confirm_only_journal_once() {
    let none = HashMap::new();
    let ps = || {
        vec![
            pair("AIL", Some(10_000), Some(1), 0),
            pair("KILO", None, None, 0),
            pair("CAKE", Some(40_000), Some(9), 0),
        ]
    };
    // Адаптер не умеет подтверждать и кадры не по расписанию.
    let (c, mut st) = (cfg(), State::default());
    let a = decide(
        &health(40_000, false, false, ps()),
        40_000,
        &c,
        &none,
        &mut st,
    );
    assert!(
        matches!(&a[..], [Action::QuietNoConfirm { tickers, .. }] if tickers == &vec!["AIL".to_string(), "KILO".to_string()]),
        "{a:?}"
    );
    assert!(
        decide(
            &health(43_000, false, false, ps()),
            43_000,
            &c,
            &none,
            &mut st
        )
        .is_empty()
    );
    // Адаптер умеет, но настройка не задана — причина в журнале.
    let mut c = cfg();
    c.quiet = QuietCfg::Off("не задана venue.quiet_confirm_min_ms".into());
    let mut st = State::default();
    let a = decide(
        &health(40_000, false, true, ps()),
        40_000,
        &c,
        &none,
        &mut st,
    );
    assert!(
        matches!(&a[..], [Action::QuietNoConfirm { why, .. }] if why.contains("quiet_confirm_min_ms")),
        "{a:?}"
    );
}
