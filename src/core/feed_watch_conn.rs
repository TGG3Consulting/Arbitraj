//! Файл: core/feed_watch_conn.rs
//! Что делает: решение 202 — шаг сторожа по пачке (одному соединению стаканов): мёртвых пар больше venue.conn_dead_pct %
//! пар соединения и по его парам нет идущей сделки — переподключить только это соединение; есть сделка — «нельзя,
//! идёт сделка» (раз, пока условие держится); не чаще venue.conn_reconnect_min_ms; больше
//! venue.conn_reconnect_max_per_hour за час — «нужен человек» (раз). Чистая логика без ожиданий.
//! Кто вызывает: core/feed_watch_logic.rs (decide).
//! От чего зависит: gateway/feed_health.rs.
//! Настройки: значения приходят в Cfg.

use std::collections::{HashMap, VecDeque};

use crate::gateway::feed_health::ConnHealth;

use super::feed_watch_logic::{Action, Cfg, State};

/// Час, мс — единица из имени настройки venue.conn_reconnect_max_per_hour (не порог).
const HOUR_MS: i64 = 3_600_000;

/// Переподключения соединения за последний час и что уже сказано.
#[derive(Debug, Default)]
pub(super) struct ConnSt {
    reconnects: VecDeque<i64>,
    blocked_told: bool,
    giveup_told: bool,
}

/// Решение по пачке: переподключить, заблокировано сделкой или предел. Ошибок нет.
pub(super) fn step(
    conn: &ConnHealth,
    dead: usize,
    now: i64,
    c: &Cfg,
    deals: &HashMap<String, Vec<String>>,
    st: &mut State,
    out: &mut Vec<Action>,
) {
    let total = conn.pairs.len();
    let cs = st.conns.entry(conn.conn.clone()).or_default();
    while cs.reconnects.front().is_some_and(|&t| now - t >= HOUR_MS) {
        cs.reconnects.pop_front();
    }
    let over = total > 0 && (dead as i64) * 100 > c.dead_pct * total as i64;
    if !over {
        cs.blocked_told = false;
        cs.giveup_told = false;
        return;
    }
    let mut ids: Vec<String> = conn
        .pairs
        .iter()
        .filter_map(|p| deals.get(&p.ticker))
        .flatten()
        .cloned()
        .collect();
    ids.sort();
    ids.dedup();
    let name = conn.conn.clone();
    if !ids.is_empty() {
        if !cs.blocked_told {
            cs.blocked_told = true;
            out.push(Action::Blocked {
                conn: name,
                dead,
                total,
                deals: ids,
            });
        }
        return;
    }
    cs.blocked_told = false;
    if cs.reconnects.len() >= c.reconnect_max_per_hour {
        if !cs.giveup_told {
            cs.giveup_told = true;
            let count = cs.reconnects.len();
            out.push(Action::GiveUp {
                conn: name,
                dead,
                total,
                count,
            });
        }
        return;
    }
    if cs
        .reconnects
        .back()
        .is_some_and(|&t| now - t < c.reconnect_min_ms)
    {
        return;
    }
    cs.reconnects.push_back(now);
    st.pairs.retain(|k, _| k.0 != conn.conn);
    out.push(Action::Reconnect {
        conn: name,
        dead,
        total,
    });
}
