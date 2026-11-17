//! Файл: core/feed_confirm_capacity_tests.rs
//! Что проверяет: решение 383 (t169) — ёмкость подтверждений тихих пар и свежесть пары, нужной торговле, на настройках
//! Gate живого бота (17.11): venue.book_stale_ms = 5000, venue.pair_dead_mult = 1, venue.pair_resub_interval_ms = 3000,
//! venue.quiet_confirm_min_ms = 30000, venue.quiet_confirm_max_per_s = 10 (было 5). Проход подтверждений — раз в
//! секунду (feed_watch.rs), ответ REST — через 150 мс (задержка взята для примера; настоящая — замер на сервере,
//! запас хода считается из замера, State::note_answers). Свежесть стакана — как в котировке адаптера (venues/gate/
//! book.rs freeze): кадр не старше venue.book_stale_ms или момент не позже срока подтверждения.
//! Проверяется: 175 молчащих пар при 10 запросах/с после первого круга все остаются подтверждёнными и «отложено
//! ведром» не растёт; при 5/с (ёмкость 5 × 30000 / 1000 = 150 < 175) — нет; нужная торговле пара не теряет свежесть при
//! любой фазе последнего кадра относительно прохода, даже когда ведро занято 175 тихими парами.
//! Чистая логика без сети и моков: снимок свежести формата стыка (gateway/feed_health.rs), ответ «книга того же номера».
//! Кто вызывает: cargo test.

use std::collections::{HashMap, HashSet};

use super::feed_confirm::{QuietCfg, apply};
use super::feed_watch_logic::{Action, Cfg, State, decide};
use crate::gateway::feed_health::{BookVersion, ConnHealth, FeedHealth, PairHealth};

/// Шаг прохода (мс) — секунда (feed_watch.rs SECOND_MS при подтверждении).
const PASS: i64 = 1_000;
/// Задержка ответа REST в примере (мс).
const LAT: i64 = 150;
/// Шаг проверки свежести между проходами (мс).
const GRID: i64 = 50;

/// Настройки Gate из задания t169; max_per_s — параметр.
fn gate(per_s: i64) -> Cfg {
    Cfg {
        book_stale_ms: 5_000,
        dead_mult: 1,
        live_ms: 40_000,
        warmup_ms: 30_000,
        attempts: 3,
        interval_ms: 3_000,
        pause_ms: 10_000,
        dead_pct: 90,
        reconnect_min_ms: 120_000,
        reconnect_max_per_hour: 3,
        quiet: QuietCfg::Confirm {
            min_ms: 30_000,
            max_per_s: per_s,
        },
    }
}

/// Снимок: соединение «m1», начато в 0, живо в now; пары (тикер, последний кадр), номер книги у всех 1.
fn snap(now: i64, pairs: &[(String, i64)]) -> FeedHealth {
    FeedHealth {
        scheduled: false,
        per_pair: true,
        confirm: true,
        conns: vec![ConnHealth {
            conn: "m1".into(),
            alive_ms: Some(now),
            started_ms: Some(0),
            pairs: pairs
                .iter()
                .map(|(t, l)| PairHealth {
                    ticker: t.clone(),
                    last_ms: Some(*l),
                    since_ms: 0,
                    last_id: Some(1),
                    last_changed_ms: None,
                })
                .collect(),
        }],
    }
}

/// Итог прогона.
struct Run {
    /// Моменты (после steady), когда стакан какой-то пары был несвежим: (тикер, момент).
    stale: Vec<(String, i64)>,
    /// «Отложено ведром» за время после steady.
    deferred: u64,
}

/// Прогон сторожа с from до to: n тихих пар (последний кадр 10 000), нужная пара HOT (кадры до hot_last, если задано).
/// Свежесть считается с момента steady. Возвращает: Run.
fn run(per_s: i64, n: usize, hot_last: Option<i64>, steady: i64, to: i64) -> Run {
    let c = gate(per_s);
    let mut st = State::default();
    if hot_last.is_some() {
        st.needed = HashSet::from(["HOT".to_string()]);
    }
    let none = HashMap::new();
    let quiet: Vec<String> = (0..n).map(|i| format!("Q{i}")).collect();
    let pairs = |x: i64| {
        let mut v: Vec<(String, i64)> = quiet.iter().map(|t| (t.clone(), 10_000)).collect();
        if let Some(f) = hot_last {
            v.push(("HOT".into(), x.min(f)));
        }
        v
    };
    let mut until: HashMap<String, i64> = HashMap::new();
    let (mut stale, mut deferred_at_steady, mut prev) = (vec![], None, None);
    let mut t = 40_000;
    while t < to {
        let checks: Vec<String> = decide(&snap(t, &pairs(t)), t, &c, &none, &mut st)
            .into_iter()
            .filter_map(|a| match a {
                Action::Check { ticker, .. } => Some(ticker),
                _ => None,
            })
            .collect();
        let mut x = t;
        while x < t + PASS {
            if x == t + LAT && !checks.is_empty() {
                let h = snap(x, &pairs(x));
                for tk in &checks {
                    let r = Ok(BookVersion {
                        id: Some(1),
                        ..Default::default()
                    });
                    for a in apply(&h, "m1", tk, r, t, x, &c, &mut st) {
                        if let Action::Confirmed {
                            ticker, until_ms, ..
                        } = a
                        {
                            until.insert(ticker, until_ms);
                        }
                    }
                }
                if let Some(p) = prev {
                    st.note_answers(p, x);
                }
            }
            if x >= steady {
                deferred_at_steady.get_or_insert(st.stats.deferred);
                for (tk, last) in pairs(x) {
                    let fresh =
                        x - last <= c.book_stale_ms || until.get(&tk).is_some_and(|&u| x <= u);
                    if !fresh {
                        stale.push((tk, x));
                    }
                }
            }
            x += GRID;
        }
        prev = Some(t);
        t += PASS;
    }
    Run {
        stale,
        deferred: st.stats.deferred - deferred_at_steady.unwrap_or(st.stats.deferred),
    }
}

#[test]
fn ten_per_second_keeps_175_quiet_pairs_confirmed() {
    // Первый круг: 175 пар по 10 в секунду — 18 проходов (40 000 … 57 000), ответы до 57 150. Дальше каждая пара
    // перепроверяется через 30 с и отвечает до срока подтверждения — ни одной дыры за 4 круга.
    let r = run(10, 175, None, 57_200, 160_000);
    assert!(
        r.stale.is_empty(),
        "дыры: {:?}",
        &r.stale[..r.stale.len().min(5)]
    );
    assert_eq!(r.deferred, 0, "ведро не отстаёт");
}

#[test]
fn five_per_second_cannot_hold_175() {
    // Ёмкость 5 × 30000 / 1000 = 150 < 175: первые подтверждения истекают, пока ведро ещё раздаёт первый круг.
    let r = run(5, 175, None, 57_200, 160_000);
    assert!(!r.stale.is_empty());
    assert!(r.deferred > 0);
}

#[test]
fn needed_pair_never_loses_freshness_under_full_bucket() {
    // HOT нужна торговле; последний кадр — в любой фазе относительно прохода (шаг 50 мс внутри секунды). Ведро занято
    // 175 тихими парами. Стакан HOT свеж всё время: кадр, затем подтверждения без разрыва (ранний порог — 5000 −
    // запас хода 1150 = 3850 мс, срок — отправка + 30000 + 1150).
    for off in (0..PASS).step_by(GRID as usize) {
        let f = 100_000 + off;
        let r = run(10, 175, Some(f), 57_200, 170_000);
        let hot: Vec<_> = r.stale.iter().filter(|(t, _)| t == "HOT").collect();
        assert!(hot.is_empty(), "кадр в {f}: HOT несвежа в {hot:?}");
        assert!(
            r.stale.is_empty(),
            "кадр в {f}: тихие несвежи {:?}",
            &r.stale[..r.stale.len().min(5)]
        );
    }
}
