//! Файл: core/feed_watch_tests.rs
//! Что проверяет: решение 202 — чистая логика сторожа пар (feed_watch_logic.rs): прогрев, мёртвая пара и круги
//! переподписки с паузой, «ожила», переподключение только пачки с долей мёртвых выше порога, блок идущей сделкой,
//! предел переподключений за час, площадка без переподписки одной пары. Без моков: вход — снимок свежести формата
//! стыка (gateway/feed_health.rs), числа — советы Тиграна (docs/FIRST_RUN.md). Площадка с кадрами по расписанию без
//! подтверждения по REST (scheduled = true, confirm = false) — поведение решения 202 не изменилось (решение 213).
//! Подтверждение тихой пары (решение 213) — feed_confirm_tests.rs.
//! Кто вызывает: cargo test.

use std::collections::HashMap;

use super::feed_confirm::QuietCfg;
use super::feed_watch_logic::{Action, Cfg, State, decide};
use crate::gateway::feed_health::{ConnHealth, FeedHealth, PairHealth};

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
        quiet: QuietCfg::Off("не задана venue.quiet_confirm_min_ms".into()),
    }
}

/// Соединение: начато в 0, живо в now; пары — (тикер, последний кадр).
fn conn(name: &str, now: i64, pairs: &[(&str, Option<i64>)]) -> ConnHealth {
    ConnHealth {
        conn: name.into(),
        alive_ms: Some(now),
        started_ms: Some(0),
        pairs: pairs
            .iter()
            .map(|(t, l)| PairHealth {
                ticker: t.to_string(),
                last_ms: *l,
                since_ms: 0,
                last_id: None,
                last_changed_ms: None,
            })
            .collect(),
    }
}

fn health(per_pair: bool, conns: Vec<ConnHealth>) -> FeedHealth {
    FeedHealth {
        scheduled: true,
        per_pair,
        confirm: false,
        conns,
    }
}

fn resubs(a: &[Action]) -> usize {
    a.iter()
        .filter(|x| matches!(x, Action::Resub { .. }))
        .count()
}

#[test]
fn warmup_then_rounds_then_recovered() {
    let (c, mut st, none) = (cfg(), State::default(), HashMap::new());
    // Прогрев соединения: ничего.
    let h = health(
        true,
        vec![conn(
            "m1",
            10_000,
            &[("QAIT", None), ("CAKE", Some(10_000))],
        )],
    );
    assert!(decide(&h, 10_000, &c, &none, &mut st).is_empty());
    // После прогрева QAIT молчит с начала (дольше 15 с) — первая попытка круга 1.
    let t = 40_000;
    let h = health(
        true,
        vec![conn("m1", t, &[("QAIT", None), ("CAKE", Some(t))])],
    );
    let a = decide(&h, t, &c, &none, &mut st);
    assert_eq!(
        a,
        vec![Action::Resub {
            conn: "m1".into(),
            ticker: "QAIT".into(),
            attempt: 1,
            round: 1
        }]
    );
    // CAKE живёт: его кадр и кадр соединения — в момент прохода.
    let at = |n: i64| {
        health(
            true,
            vec![conn("m1", n, &[("QAIT", None), ("CAKE", Some(n))])],
        )
    };
    let mut run = |n: i64| decide(&at(n), n, &c, &none, &mut st);
    // Раньше интервала — ничего; по интервалу — попытки 2 и 3; потом пауза.
    assert!(run(t + 1_000).is_empty());
    assert_eq!(resubs(&run(t + 3_000)), 1);
    assert_eq!(resubs(&run(t + 6_000)), 1);
    assert_eq!(resubs(&run(t + 9_000)), 0, "круг окончен — пауза");
    assert_eq!(resubs(&run(t + 30_000)), 0, "в паузе — ничего");
    let a = run(t + 9_000 + 60_000);
    assert!(
        matches!(
            &a[..],
            [Action::Resub {
                attempt: 1,
                round: 2,
                ..
            }]
        ),
        "{a:?}"
    );
    // Кадр пришёл — «ожила».
    let n = t + 70_000;
    let h = health(
        true,
        vec![conn("m1", n, &[("QAIT", Some(n)), ("CAKE", Some(n))])],
    );
    assert_eq!(
        decide(&h, n, &c, &none, &mut st),
        vec![Action::Recovered {
            conn: "m1".into(),
            ticker: "QAIT".into()
        }]
    );
}

/// Прогнать два круга попыток по всем мёртвым парам. Возвращает: последние действия.
fn two_rounds(
    h: &FeedHealth,
    c: &Cfg,
    deals: &HashMap<String, Vec<String>>,
    st: &mut State,
    t0: i64,
) -> Vec<Action> {
    let mut t = t0;
    let mut last = vec![];
    for _ in 0..2 {
        for _ in 0..=c.attempts {
            last = decide(h, t, c, deals, st);
            t += c.interval_ms;
        }
        t += c.pause_ms;
    }
    last.extend(decide(h, t, c, deals, st));
    last
}

#[test]
fn reconnect_only_dead_batch_and_blocked_by_deal() {
    let c = cfg();
    let t = 100_000;
    // m1: 2 из 4 мёртвы (50% > 30) — переподключение; m2: все живы — не трогается.
    let pairs = |n: i64| vec![("A", None), ("B", None), ("C", Some(n)), ("D", Some(n))];
    let mk = |n: i64| {
        health(
            true,
            vec![
                conn("m1", n, &pairs(n)),
                conn("m2", n, &[("X", Some(n)), ("Y", Some(n))]),
            ],
        )
    };
    // Время не стоит: кадры живых пар обновляются на каждом проходе (через замыкание на t).
    let mut st = State::default();
    let none = HashMap::new();
    let h = mk(t + 300_000);
    let a = two_rounds(&h, &c, &none, &mut st, t);
    assert!(
        a.iter()
            .any(|x| matches!(x, Action::Reconnect { conn, dead: 2, total: 4 } if conn == "m1")),
        "{a:?}"
    );
    assert!(
        !a.iter()
            .any(|x| matches!(x, Action::Reconnect { conn, .. } if conn == "m2"))
    );
    // То же, но по паре C соединения m1 идёт сделка — блок, без переподключения.
    let mut st = State::default();
    let deals = HashMap::from([("C".to_string(), vec!["d1".to_string()])]);
    let a = two_rounds(&h, &c, &deals, &mut st, t);
    assert!(
        a.iter().any(
            |x| matches!(x, Action::Blocked { deals, .. } if deals == &vec!["d1".to_string()])
        )
    );
    assert!(!a.iter().any(|x| matches!(x, Action::Reconnect { .. })));
}

#[test]
fn hour_limit_and_no_per_pair() {
    let mut c = cfg();
    c.reconnect_min_ms = 1;
    c.reconnect_max_per_hour = 2;
    let none = HashMap::new();
    let mut st = State::default();
    // Без переподписки пары: мёртвые считаются сразу — переподключение, без Resub.
    let mut reconnects = 0;
    let mut gave_up = false;
    for k in 0..4 {
        let n = 100_000 + k * 1_000;
        let h = FeedHealth {
            scheduled: true,
            per_pair: false,
            confirm: false,
            conns: vec![conn("m1", n, &[("A", None), ("B", Some(n))])],
        };
        for a in decide(&h, n, &c, &none, &mut st) {
            match a {
                Action::Reconnect { .. } => reconnects += 1,
                Action::GiveUp { count: 2, .. } => gave_up = true,
                Action::Resub { .. } => panic!("переподписки пары у площадки нет"),
                _ => {}
            }
        }
    }
    assert_eq!(reconnects, 2, "не больше venue.conn_reconnect_max_per_hour");
    assert!(gave_up, "сверх предела — «нужен человек»");
}
