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
//! Решение 213 (Тигран, 07.11.2026: «тишина пары ≠ смерть») — что делать с молчащей по порогу парой, решает режим:
//! - адаптер умеет подтверждать по REST (FeedHealth.confirm) и заданы venue.quiet_confirm_* — спросить биржу
//!   (Action::Check, feed_confirm.rs): книга не менялась — тихая, подтверждена; новее — кадры потеряны → круги
//!   переподписки как выше (после паузы круга — снова проверка, а не новый круг вслепую);
//! - иначе, если по документации кадры идут по расписанию (FeedHealth.scheduled) — как в решении 202;
//! - иначе — только журнал (раз на пару), не переподписывать вслепую.
//!
//! Кто вызывает: core/feed_watch.rs. Шаг по пачке — feed_watch_conn.rs; подтверждение — feed_confirm.rs.
//! От чего зависит: gateway/feed_health.rs, feed_confirm.rs, feed_watch_action.rs (действия).
//! Настройки: значения приходят в Cfg (читает feed_watch.rs).

use std::collections::{HashMap, HashSet};

use crate::gateway::feed_health::FeedHealth;

use super::feed_confirm::{self as confirm, QuietCfg};
pub use super::feed_watch_action::Action;

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
    /// Решение 213: подтверждение тихой пары по REST.
    pub quiet: QuietCfg,
}

/// Что делать с молчащей парой (решение 213).
enum Mode {
    /// Спросить биржу: venue.quiet_confirm_min_ms, venue.quiet_confirm_max_per_s.
    Confirm(i64, i64),
    /// Кадры по расписанию — переподписка (решение 202).
    Resub,
    /// Только журнал — причина.
    Journal(String),
}

/// Режим площадки. Ошибок нет.
fn mode(h: &FeedHealth, c: &Cfg) -> Mode {
    match (&c.quiet, h.confirm) {
        (QuietCfg::Confirm { min_ms, max_per_s }, true) => Mode::Confirm(*min_ms, *max_per_s),
        _ if h.scheduled => Mode::Resub,
        (QuietCfg::Off(why), true) => Mode::Journal(format!("подтверждение по REST выключено: {why}")),
        (_, false) => Mode::Journal(
            "адаптер не подтверждает стакан по REST, а кадры биржи по документации не по расписанию".into(),
        ),
    }
}

#[derive(Debug, Default, Clone)]
pub(super) struct PairSt {
    attempts: u32,
    rounds: u32,
    pub(super) next_at: i64,
}

impl PairSt {
    /// Пара в круге переподписки или в паузе после неудачного круга (для сводки журнала, feed_watch_log.rs).
    /// Ошибок нет.
    pub(super) fn busy(&self) -> bool {
        self.attempts > 0 || self.rounds > 0
    }
}

/// Состояние сторожа одной площадки между проходами.
#[derive(Debug, Default)]
pub struct State {
    pub(super) pairs: HashMap<(String, String), PairSt>,
    pub(super) conns: HashMap<String, super::feed_watch_conn::ConnSt>,
    /// Решение 213: проверки тихих пар и ведро запросов площадки.
    pub(super) checks: HashMap<(String, String), confirm::CheckSt>,
    pub(super) bucket: confirm::Bucket,
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
    st.checks.retain(|k, _| present.contains(k));
    let mode = mode(h, c);
    st.conns.retain(|k, _| h.conns.iter().any(|x| &x.conn == k));
    for conn in &h.conns {
        let live = conn.alive_ms.is_some_and(|a| now - a <= c.live_ms);
        let warm = conn.started_ms.is_none_or(|s| now < s + c.warmup_ms);
        if !live || warm {
            continue;
        }
        let mut dead = 0usize;
        let mut quiet_new = vec![];
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
                // Решение 213: замолчит снова — проверка сразу (ведро позволит), прежний срок не держит; запомненный
                // ответ «кадры потеряны» (дополнение 10.11, feed_rest_ref.rs) к новой тишине не относится.
                if let Some(cs) = st.checks.get_mut(&key) {
                    cs.next_at = 0;
                    cs.lost_ref = None;
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
            let silent = dead_pair(p, now, c, dead_after);
            match &mode {
                _ if in_round => round_step(st, &key, now, c, &mut out),
                Mode::Resub if silent => round_step(st, &key, now, c, &mut out),
                Mode::Confirm(min_ms, per_s) if silent => {
                    confirm::want(st, &key, now, *min_ms, *per_s, &mut out)
                }
                Mode::Journal(_) if silent && confirm::note_quiet(st, &key) => {
                    quiet_new.push(p.ticker.clone())
                }
                _ => {}
            }
            if st
                .pairs
                .get(&key)
                .is_some_and(|s| s.rounds >= ROUNDS_FOR_CONN)
            {
                dead += 1;
            }
        }
        if let (Mode::Journal(why), false) = (&mode, quiet_new.is_empty()) {
            out.push(Action::QuietNoConfirm {
                conn: conn.conn.clone(),
                why: why.clone(),
                tickers: quiet_new,
            });
        }
        super::feed_watch_conn::step(conn, dead, now, c, deals, st, &mut out);
    }
    out
}

/// Шаг круга переподписки пары (решение 202): очередная попытка по интервалу или конец круга и пауза. Ошибок нет.
fn round_step(st: &mut State, key: &(String, String), now: i64, c: &Cfg, out: &mut Vec<Action>) {
    let s = st.pairs.entry(key.clone()).or_default();
    if now < s.next_at {
        return;
    }
    if s.attempts < c.attempts {
        s.attempts += 1;
        s.next_at = now + c.interval_ms;
        out.push(Action::Resub {
            conn: key.0.clone(),
            ticker: key.1.clone(),
            attempt: s.attempts,
            round: s.rounds + 1,
        });
    } else {
        s.rounds += 1;
        s.attempts = 0;
        s.next_at = now + c.pause_ms;
    }
}

/// Решение 213: кадры потеряны (ответ биржи) — начать круг переподписки, если пара не в круге и не в паузе.
/// Ошибок нет.
pub(super) fn start_round(
    st: &mut State,
    key: &(String, String),
    now: i64,
    c: &Cfg,
    out: &mut Vec<Action>,
) {
    if st.pairs.get(key).is_none_or(|s| s.attempts == 0) {
        round_step(st, key, now, c, out);
    }
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
