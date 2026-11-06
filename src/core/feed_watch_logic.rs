//! Файл: core/feed_watch_logic.rs
//! Что делает: решение 202 (Тигран, 06.11.2026) — чистая логика сторожа пар: по снимку свежести соединений площадки
//! (gateway/feed_health.rs), настройкам и монетам идущих сделок решает, какую пару переподписать, какое соединение
//! (пачку) переподключить, о чём сообщить. Без ожиданий и ввода-вывода — проверяется тестами (feed_watch_tests.rs).
//! Правила:
//! - мёртвая пара: соединение живо (кадр соединения не старше feed.stale_after_ms) и прогрев прошёл, а кадра пары нет
//!   дольше venue.book_stale_ms × venue.pair_dead_mult (от последнего кадра; кадров не было — от (пере)подписки);
//! - прогрев venue.feed_warmup_ms после начала сессии соединения и после (пере)подписки пары — пара не мёртвая;
//! - по паре: venue.pair_resub_attempts попыток с интервалом venue.pair_resub_interval_ms; не ожила — пауза
//!   venue.pair_resub_pause_ms, снова попытки, по кругу; ожила (свежий кадр) — счётчики сброшены;
//! - пачка: мёртвых пар (после двух неудачных кругов; где переподписки одной пары нет — сразу) больше
//!   venue.conn_dead_pct % пар соединения и нет идущей сделки по паре этого соединения — переподключить только его;
//!   не чаще venue.conn_reconnect_min_ms, не больше venue.conn_reconnect_max_per_hour за час, сверх — «нужен человек».
//!
//! Кто вызывает: core/feed_watch.rs. Шаг по пачке — feed_watch_conn.rs.
//! От чего зависит: gateway/feed_health.rs.
//! Настройки: значения приходят в Cfg (читает feed_watch.rs).

use std::collections::{HashMap, HashSet};

use crate::gateway::feed_health::FeedHealth;

/// Кругов попыток по паре, после которых пара считается в доле мёртвых пачки — из ТЗ Тиграна («после двух неудачных
/// раундов»), правило, а не порог: число попыток и паузы — настройки.
const ROUNDS_FOR_CONN: u32 = 2;

/// Настройки сторожа площадки (мс, штуки, проценты).
#[derive(Debug, Clone)]
pub struct Cfg {
    pub book_stale_ms: i64,
    pub dead_mult: i64,
    pub live_ms: i64,
    pub warmup_ms: i64,
    pub attempts: u32,
    pub interval_ms: i64,
    pub pause_ms: i64,
    pub dead_pct: i64,
    pub reconnect_min_ms: i64,
    pub reconnect_max_per_hour: usize,
}

/// Что сделать.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Переподписать пару: попытка в круге (с 1) и номер круга (с 1).
    Resub {
        conn: String,
        ticker: String,
        attempt: u32,
        round: u32,
    },
    /// Пара ожила после попыток.
    Recovered { conn: String, ticker: String },
    /// Переподключить соединение: мёртвых пар из всех.
    Reconnect {
        conn: String,
        dead: usize,
        total: usize,
    },
    /// Переподключить нельзя — по паре соединения идёт сделка (id сделок).
    Blocked {
        conn: String,
        dead: usize,
        total: usize,
        deals: Vec<String>,
    },
    /// Предел переподключений за час исчерпан — нужен человек.
    GiveUp {
        conn: String,
        dead: usize,
        total: usize,
        count: usize,
    },
}

#[derive(Debug, Default, Clone)]
pub(super) struct PairSt {
    attempts: u32,
    rounds: u32,
    next_at: i64,
}

/// Состояние сторожа одной площадки между проходами.
#[derive(Debug, Default)]
pub struct State {
    pub(super) pairs: HashMap<(String, String), PairSt>,
    pub(super) conns: HashMap<String, super::feed_watch_conn::ConnSt>,
}

/// Один проход. Принимает: снимок свежести, сейчас (unix мс), настройки, монеты идущих сделок площадки
/// (тикер → id сделок), состояние. Возвращает: действия по порядку. Ошибок нет.
pub fn decide(
    h: &FeedHealth,
    now: i64,
    c: &Cfg,
    deals: &HashMap<String, Vec<String>>,
    st: &mut State,
) -> Vec<Action> {
    let mut out = vec![];
    let dead_after = c.book_stale_ms * c.dead_mult;
    let present: HashSet<(String, String)> = h
        .conns
        .iter()
        .flat_map(|k| k.pairs.iter().map(|p| (k.conn.clone(), p.ticker.clone())))
        .collect();
    st.pairs.retain(|k, _| present.contains(k));
    st.conns.retain(|k, _| h.conns.iter().any(|x| &x.conn == k));
    for conn in &h.conns {
        let live = conn.alive_ms.is_some_and(|a| now - a <= c.live_ms);
        let warm = conn.started_ms.is_none_or(|s| now < s + c.warmup_ms);
        if !live || warm {
            continue;
        }
        let mut dead = 0usize;
        for p in &conn.pairs {
            let key = (conn.conn.clone(), p.ticker.clone());
            let fresh = p.last_ms.is_some_and(|l| now - l <= dead_after);
            if fresh {
                if st
                    .pairs
                    .remove(&key)
                    .is_some_and(|s| s.attempts > 0 || s.rounds > 0)
                {
                    out.push(Action::Recovered {
                        conn: conn.conn.clone(),
                        ticker: p.ticker.clone(),
                    });
                }
                continue;
            }
            if !h.per_pair {
                if dead_pair(p, now, c, dead_after) {
                    dead += 1;
                }
                continue;
            }
            // Идёт круг попыток — следующая по расписанию (venue.pair_resub_interval_ms), пока не придёт кадр;
            // вне круга пара должна снова оказаться мёртвой (после прогрева).
            let in_round = st.pairs.get(&key).is_some_and(|s| s.attempts > 0);
            if in_round || dead_pair(p, now, c, dead_after) {
                let s = st.pairs.entry(key.clone()).or_default();
                if now >= s.next_at {
                    if s.attempts < c.attempts {
                        s.attempts += 1;
                        s.next_at = now + c.interval_ms;
                        out.push(Action::Resub {
                            conn: conn.conn.clone(),
                            ticker: p.ticker.clone(),
                            attempt: s.attempts,
                            round: s.rounds + 1,
                        });
                    } else {
                        s.rounds += 1;
                        s.attempts = 0;
                        s.next_at = now + c.pause_ms;
                    }
                }
            }
            if st
                .pairs
                .get(&key)
                .is_some_and(|s| s.rounds >= ROUNDS_FOR_CONN)
            {
                dead += 1;
            }
        }
        super::feed_watch_conn::step(conn, dead, now, c, deals, st, &mut out);
    }
    out
}

/// Мёртвая ли пара вне круга попыток: прогрев прошёл и кадра нет дольше порога (от последнего кадра или (пере)подписки —
/// что позже). Ошибок нет.
fn dead_pair(
    p: &crate::gateway::feed_health::PairHealth,
    now: i64,
    c: &Cfg,
    dead_after: i64,
) -> bool {
    if now < p.since_ms + c.warmup_ms {
        return false;
    }
    let from = p.last_ms.unwrap_or(0).max(p.since_ms);
    now - from > dead_after
}
