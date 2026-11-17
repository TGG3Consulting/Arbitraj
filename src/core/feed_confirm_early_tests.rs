//! Файл: core/feed_confirm_early_tests.rs
//! Что проверяет: t163 (решение 367, для всех бирж) — сторож спрашивает биржу о тихой паре ДО того, как котировка
//! признает стакан устаревшим (тишина больше venue.book_stale_ms − venue.pair_resub_interval_ms), ответ на такой ранний
//! запрос не выбрасывается (раньше — «кадр моложе book_stale_ms × pair_dead_mult» → ответ терялся, пара между
//! book_stale_ms и book_stale_ms × pair_dead_mult стояла в отказе «кадра стакана пары нет»), подтверждение покрывает
//! момент, когда кадр стал бы старше book_stale_ms; повторный запрос — не раньше venue.quiet_confirm_min_ms, после нового
//! кадра — сразу; кадр после отправки — ответ не нужен; без подтверждения (журнал, переподписка 202) — как раньше.
//! Доработка по ревью Н2: ранняя проверка в приоритете — у пар, нужных торговле (State.needed / идущая сделка);
//! решение 383: у остальных — из остатка ведра (после обычных, не «отложены»); ведро — по сроку ожидания, нужные первыми, старые ждущие не голодают; первое «книга новее» ранней
//! проверки — без переподписки (повторная сверка), второе подряд — переподписка; кадр между ними — сверка снята;
//! сводка журнала показывает «отложено ведром» и исходы проверок нужных пар.
//! Повторное ревью t163: Б1 — первое «книга новее» снимает подтверждение сразу (Action::Unconfirm); Н4 — ошибка REST
//! между двумя «новее» сверку не снимает (переподписка не откладывается); Н1 — пара, ставшая нужной, остаётся в
//! приоритете, пока книгу не подтвердят или не придёт кадр, даже если сигнал её больше не держит.
//! Вход — снимок свежести формата стыка (gateway/feed_health.rs), числа — советы docs/FIRST_RUN.md, как в
//! feed_confirm_tests.rs. Без моков сети.
//! Кто вызывает: cargo test.

use std::collections::{HashMap, HashSet};

use super::feed_confirm::{QuietCfg, apply};
use super::feed_watch_log::LogGate;
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

/// Снимок с несколькими парами: (тикер, последний кадр), номер книги 1.
fn many(now: i64, pairs: &[(&str, i64)]) -> FeedHealth {
    let mut h = health(now, 0, 1);
    h.conns[0].pairs = pairs
        .iter()
        .map(|(t, l)| PairHealth {
            ticker: (*t).into(),
            last_ms: Some(*l),
            since_ms: 0,
            last_id: Some(1),
            last_changed_ms: None,
        })
        .collect();
    h
}

/// Состояние сторожа, где перечисленные тикеры нужны торговле (feed_need.rs).
fn need(tickers: &[&str]) -> State {
    State {
        needed: tickers
            .iter()
            .map(|t| t.to_string())
            .collect::<HashSet<_>>(),
        ..Default::default()
    }
}

fn by_id(id: i64) -> Result<BookVersion, String> {
    Ok(BookVersion {
        id: Some(id),
        ..Default::default()
    })
}

fn unconfirm() -> Action {
    Action::Unconfirm {
        conn: "m1".into(),
        ticker: "GMT".into(),
    }
}

fn check() -> Action {
    Action::Check {
        conn: "m1".into(),
        ticker: "GMT".into(),
    }
}

#[test]
fn quiet_pair_confirmed_before_quote_calls_it_stale() {
    let (c, mut st, none) = (cfg(), need(&["GMT"]), HashMap::new());
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
    let (c, mut st, none) = (cfg(), need(&["GMT"]), HashMap::new());
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

#[test]
fn not_needed_pair_early_from_spare_bucket_and_deal_counts_as_needed() {
    // Решение 383: пара не нужна торговле, тишина 6 с (ранний порог 2 с пройден, порог мёртвой — 15 с): ранняя
    // проверка из ОСТАТКА ведра — после обычных, «отложены ведром» не считается. До 383 — ждала 15 с.
    let mut c = cfg();
    c.quiet = QuietCfg::Confirm {
        min_ms: 30_000,
        max_per_s: 1,
    };
    let (mut st, none) = (State::default(), HashMap::new());
    let n = |a: &[Action]| -> Vec<String> {
        a.iter()
            .filter_map(|x| match x {
                Action::Check { ticker, .. } => Some(ticker.clone()),
                _ => None,
            })
            .collect()
    };
    let h = |now| many(now, &[("GMT", 40_000), ("OLD", 10_000)]);
    assert_eq!(
        n(&decide(&h(46_000), 46_000, &c, &none, &mut st)),
        vec!["OLD"]
    );
    assert_eq!(st.stats.deferred, 0, "ранняя из остатка не «отложена»");
    assert_eq!(
        n(&decide(&h(47_000), 47_000, &c, &none, &mut st)),
        vec!["GMT"]
    );
    assert_eq!(st.stats.spare_checks, 1);
    assert_eq!(
        st.stats.early_checks, 0,
        "не нужная торговле — не в счётчике нужных"
    );
    // Идущая сделка по монете — пара нужна торговле без State.needed.
    let mut st = State::default();
    let deals = HashMap::from([("GMT".to_string(), vec!["d1".to_string()])]);
    assert_eq!(
        decide(
            &health(46_000, 40_000, 100),
            46_000,
            &cfg(),
            &deals,
            &mut st
        ),
        vec![check()]
    );
    assert_eq!(st.stats.early_checks, 1);
}

#[test]
fn first_early_newer_book_rechecks_without_resub() {
    let (c, mut st, none) = (cfg(), need(&["GMT"]), HashMap::new());
    assert_eq!(
        decide(&health(40_600, 38_500, 100), 40_600, &c, &none, &mut st),
        vec![check()]
    );
    // На бирже 101 — книга изменилась, кадр мог быть в пути: без переподписки и без журнала, повторная сверка.
    let a = apply(
        &health(40_700, 38_500, 100),
        "m1",
        "GMT",
        by_id(101),
        40_600,
        40_700,
        &c,
        &mut st,
    );
    assert_eq!(
        a,
        vec![unconfirm()],
        "первое «новее» — подтверждение снимается сразу (Б1), без переподписки"
    );
    // Кадр пришёл (101) — сверка снята, проверять нечего.
    assert!(decide(&health(42_500, 41_000, 101), 42_500, &c, &none, &mut st).is_empty());
    // Снова тихо; ранняя проверка — «новее», затем второй проход — опять «новее» без кадра: кадры потеряны.
    assert_eq!(
        decide(&health(43_200, 41_000, 101), 43_200, &c, &none, &mut st),
        vec![check()]
    );
    let a = apply(
        &health(43_300, 41_000, 101),
        "m1",
        "GMT",
        by_id(103),
        43_200,
        43_300,
        &c,
        &mut st,
    );
    assert_eq!(
        a,
        vec![unconfirm()],
        "первое «новее» — подтверждение снимается сразу (Б1), без переподписки"
    );
    assert_eq!(
        decide(&health(46_200, 41_000, 101), 46_200, &c, &none, &mut st),
        vec![check()],
        "повторная сверка — следующим проходом, не через venue.quiet_confirm_min_ms"
    );
    let a = apply(
        &health(46_300, 41_000, 101),
        "m1",
        "GMT",
        by_id(103),
        46_200,
        46_300,
        &c,
        &mut st,
    );
    assert!(
        matches!(
            &a[..],
            [Action::Lost { .. }, Action::Resub { attempt: 1, .. }]
        ),
        "{a:?}"
    );
    assert_eq!(st.stats.early_suspect, 2);
    assert_eq!(st.stats.early_lost, 1);
}

#[test]
fn bucket_serves_needed_first_and_old_waiters_do_not_starve() {
    let mut c = cfg();
    c.quiet = QuietCfg::Confirm {
        min_ms: 30_000,
        max_per_s: 1,
    };
    let (mut st, none) = (need(&["HOT"]), HashMap::new());
    let n = |a: &[Action]| -> Vec<String> {
        a.iter()
            .filter_map(|x| match x {
                Action::Check { ticker, .. } => Some(ticker.clone()),
                _ => None,
            })
            .collect()
    };
    // OLD молчит давно (ждёт с 25 с), HOT нужна торговле и молчит 2,5 с: ведро на 1 запрос — HOT первой.
    let h = |now| many(now, &[("OLD", 10_000), ("HOT", now - 2_500)]);
    assert_eq!(
        n(&decide(&h(40_000), 40_000, &c, &none, &mut st)),
        vec!["HOT"]
    );
    assert_eq!(st.stats.deferred, 1, "OLD отложена ведром");
    // Следующая секунда — OLD (HOT уже спрошена, срок 30 с).
    assert_eq!(
        n(&decide(&h(41_000), 41_000, &c, &none, &mut st)),
        vec!["OLD"]
    );
    // Десять нужных пар молчат с 37,5 с и старая ждущая OLD (ждёт с 25 с): нужные — первыми, по одной в секунду, затем
    // OLD; каждая молчащая пара спрошена (её срок не меняется, пока нет кадра — шапка feed_confirm_queue.rs).
    let hot: Vec<String> = (0..10).map(|i| format!("H{i}")).collect();
    let run = |frames: bool| {
        let mut st = State {
            needed: hot.iter().cloned().collect(),
            ..Default::default()
        };
        let (mut served, mut old_at) = (HashSet::new(), None);
        for k in 0..30 {
            let now = 40_000 + k * 1_000;
            // frames: у нужных кадр каждый проход (тишина 2,5 с) — их ранние проверки повторяются после каждого кадра.
            let last = if frames { now - 2_500 } else { 37_500 };
            let mut ps: Vec<(&str, i64)> = hot.iter().map(|t| (t.as_str(), last)).collect();
            ps.push(("OLD", 10_000));
            let got = n(&decide(&many(now, &ps), now, &c, &none, &mut st));
            if old_at.is_none() && got.iter().any(|t| t == "OLD") {
                old_at = Some(now);
            }
            served.extend(got);
        }
        (served, old_at)
    };
    let (served, old_at) = run(false);
    assert_eq!(served.len(), 11, "{served:?}");
    assert_eq!(old_at, Some(50_000), "десять нужных за 10 с, затем OLD");
    // Нужные пары с кадрами всё время просят проверку снова, но их новые сроки — от кадра: после срока OLD
    // (25 с) + quiet_confirm_min_ms (30 с) они позже срока OLD — OLD получает ведро, не голодает.
    let (_, old_at) = run(true);
    assert!(old_at.is_some_and(|t| t <= 57_000), "{old_at:?}");
}

#[test]
fn summary_shows_deferred_and_early_outcomes() {
    let mut c = cfg();
    c.quiet = QuietCfg::Confirm {
        min_ms: 30_000,
        max_per_s: 1,
    };
    let (mut st, none) = (need(&["GMT", "ZIG"]), HashMap::new());
    let h = many(40_600, &[("GMT", 38_500), ("ZIG", 38_500)]);
    decide(&h, 40_600, &c, &none, &mut st);
    let mut lg = LogGate::default();
    lg.every = Some(60_000);
    let s = lg.summary(&h, 40_600, &c, &st).expect("сводка");
    assert!(s.contains("отложено ведром 1"), "{s}");
    assert!(s.contains("проверок нужных торговле пар 1"), "{s}");
}

#[test]
fn rest_error_between_newer_answers_does_not_delay_resub() {
    let (c, mut st, none) = (cfg(), need(&["GMT"]), HashMap::new());
    let h = |n| health(n, 38_500, 100);
    assert_eq!(
        decide(&h(40_600), 40_600, &c, &none, &mut st),
        vec![check()]
    );
    let a = apply(
        &h(40_700),
        "m1",
        "GMT",
        by_id(101),
        40_600,
        40_700,
        &c,
        &mut st,
    );
    assert_eq!(a, vec![unconfirm()]);
    // Повторная сверка — ошибка REST: сверка остаётся.
    assert_eq!(
        decide(&h(43_600), 43_600, &c, &none, &mut st),
        vec![check()]
    );
    let err = Err("HTTP 503".to_string());
    let a = apply(&h(43_700), "m1", "GMT", err, 43_600, 43_700, &c, &mut st);
    assert!(matches!(&a[..], [Action::CheckFailed { .. }]), "{a:?}");
    // Следующий ответ «новее» — уже второй: кадры потеряны и переподписка.
    assert_eq!(
        decide(&h(73_700), 73_700, &c, &none, &mut st),
        vec![check()]
    );
    let a = apply(
        &h(73_800),
        "m1",
        "GMT",
        by_id(101),
        73_700,
        73_800,
        &c,
        &mut st,
    );
    assert!(
        matches!(
            &a[..],
            [Action::Lost { .. }, Action::Resub { attempt: 1, .. }]
        ),
        "{a:?}"
    );
}

#[test]
fn needed_pair_keeps_priority_until_confirmed_or_frame() {
    // Снимок many(): у всех пар номер книги последнего кадра — 1 (сравнение REST — с ним).
    let mut c = cfg();
    c.quiet = QuietCfg::Confirm {
        min_ms: 30_000,
        max_per_s: 1,
    };
    let none = HashMap::new();
    let h = |n, gmt_last| many(n, &[("OLD", 10_000), ("GMT", gmt_last)]);
    let n = |a: &[Action]| -> Vec<String> {
        a.iter()
            .filter_map(|x| match x {
                Action::Check { ticker, .. } => Some(ticker.clone()),
                _ => None,
            })
            .collect()
    };
    // 1) Снятие приоритета новым кадром. GMT нужна, тишина 2,1 с — ранняя проверка, ведро (1 запрос/с) ей, OLD
    // (ждёт с 25 с) отложена.
    let mut st = need(&["GMT"]);
    assert_eq!(
        n(&decide(&h(40_600, 38_500), 40_600, &c, &none, &mut st)),
        vec!["GMT"]
    );
    // GMT выпала из «нужных» (сигнал без цены). Ответ: книга 2 новее кадра 1 — первое «новее»: подтверждение снято.
    st.needed.clear();
    let a = apply(
        &h(40_700, 38_500),
        "m1",
        "GMT",
        by_id(2),
        40_600,
        40_700,
        &c,
        &mut st,
    );
    assert_eq!(a, vec![unconfirm()]);
    // Через секунду ведро снова даёт запрос: повторная сверка GMT — раньше OLD, хотя GMT нет в State.needed (sticky).
    assert_eq!(
        n(&decide(&h(41_600, 38_500), 41_600, &c, &none, &mut st)),
        vec!["GMT"]
    );
    // Пришёл кадр GMT (42 000) — приоритет и сверка сняты, GMT свежа; ведро — OLD.
    assert_eq!(
        n(&decide(&h(42_600, 42_000), 42_600, &c, &none, &mut st)),
        vec!["OLD"]
    );

    // 2) Снятие приоритета подтверждением. GMT нужна — ведро ей; выпала из «нужных»; биржа: книга 1 = кадр 1 —
    // подтверждена; через секунду ведро — OLD (GMT больше не в приоритете и свежа по подтверждению).
    let mut st = need(&["GMT"]);
    assert_eq!(
        n(&decide(&h(40_600, 38_500), 40_600, &c, &none, &mut st)),
        vec!["GMT"]
    );
    st.needed.clear();
    let a = apply(
        &h(40_700, 38_500),
        "m1",
        "GMT",
        by_id(1),
        40_600,
        40_700,
        &c,
        &mut st,
    );
    assert!(matches!(&a[..], [Action::Confirmed { .. }]), "{a:?}");
    assert_eq!(
        n(&decide(&h(41_600, 38_500), 41_600, &c, &none, &mut st)),
        vec!["OLD"]
    );
}
