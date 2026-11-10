//! Файл: core/feed_watch_log.rs
//! Что делает: дополнение к решениям 202/213 (Тигран 10.11: согласовано) — журнал видимости сторожа пар для всех бирж:
//! по журналу видно, что сторож площадки жив и что он решил по каждой паре. Три вида строк:
//! - «Сторож пар работает» — один раз, когда сторож площадки включился (все настройки решения 202 заданы): соединений,
//!   пар, режим подтверждения тихих пар, задана ли частота сводки; пишется всегда, без настройки;
//! - сводка прохода — не чаще venue.feed_watch_log_ms на площадку: пары свежие / молчат и что о них решено (тихая
//!   подтверждена, книга не менялась с переподписки, кадры потеряны, проверка не удалась, без подтверждения, в кругах
//!   переподписки), сколько переподписок, оживших пар и переподключений с прошлой сводки; не задана — сводки нет;
//! - строка решения по паре (мёртвая — переподписка, тихая подтверждена, книга не менялась, кадры потеряны, проверка не
//!   удалась, ожила): venue.feed_watch_log_ms не задана — как раньше (при смене состояния пары, переподписка — первая
//!   попытка круга, ожила — каждый раз); задана — одна строка вида решения по паре не чаще раза в столько мс
//!   (повторы того же решения тоже видны, но не засоряют журнал).
//!
//! Чистая логика без ввода-вывода; пишет core/feed_watch_act.rs (строки пар) и core/feed_watch.rs (сводка).
//! Кто вызывает: core/feed_watch.rs, core/feed_watch_act.rs.
//! От чего зависит: gateway/feed_health.rs, feed_watch_logic.rs (Cfg, State), feed_confirm.rs (QuietCfg, Told),
//! feed_watch_action.rs (Action).
//! Настройки: venue.feed_watch_log_ms (по площадке; читает feed_watch.rs; значения по умолчанию нет).

use std::collections::HashMap;

use crate::gateway::feed_health::FeedHealth;

use super::feed_confirm::{QuietCfg, Told};
use super::feed_watch_action::Action;
use super::feed_watch_logic::{Cfg, State};

/// Частота строк журнала сторожа одной площадки и счётчики с прошлой сводки.
#[derive(Debug, Default)]
pub struct LogGate {
    /// venue.feed_watch_log_ms; None — не задана.
    pub every: Option<i64>,
    /// (соединение, тикер, вид решения) → когда строка писалась (unix мс).
    last: HashMap<(String, String, &'static str), i64>,
    summary_at: Option<i64>,
    resubs: usize,
    recovered: usize,
    reconnects: usize,
    announced: bool,
}

impl LogGate {
    /// Писать ли строку решения по паре. Принимает: соединение, тикер, вид решения, прежнее правило (смена состояния,
    /// первая попытка круга), сейчас (unix мс). Возвращает: true — писать. Ошибок нет.
    pub fn pair(
        &mut self,
        conn: &str,
        ticker: &str,
        kind: &'static str,
        changed: bool,
        now: i64,
    ) -> bool {
        let Some(every) = self.every else {
            return changed;
        };
        let key = (conn.to_string(), ticker.to_string(), kind);
        if self.last.get(&key).is_some_and(|&t| now - t < every) {
            return false;
        }
        self.last.insert(key, now);
        true
    }

    /// Учесть действие в счётчиках сводки. Ошибок нет.
    pub fn count(&mut self, a: &Action) {
        match a {
            Action::Resub { .. } => self.resubs += 1,
            Action::Recovered { .. } => self.recovered += 1,
            Action::Reconnect { .. } => self.reconnects += 1,
            _ => {}
        }
    }

    /// Сторож площадки выключен (настройка не задана) — при включении снова строка «работает». Ошибок нет.
    pub fn off(&mut self) {
        self.announced = false;
    }

    /// Строка «Сторож пар работает» — один раз после включения. Ошибок нет.
    pub fn announce(&mut self, h: &FeedHealth, c: &Cfg) -> Option<String> {
        if self.announced {
            return None;
        }
        self.announced = true;
        let pairs: usize = h.conns.iter().map(|k| k.pairs.len()).sum();
        let every = match self.every {
            Some(ms) => format!("сводка — не чаще раза в {ms} мс (venue.feed_watch_log_ms)"),
            None => "сводки нет: venue.feed_watch_log_ms не задана (строки по парам — при смене состояния)".into(),
        };
        Some(format!(
            "Сторож пар работает: соединений {}, пар {pairs}; {}; {every}",
            h.conns.len(),
            mode(h, c)
        ))
    }

    /// Сводка прохода — не чаще venue.feed_watch_log_ms. Принимает: снимок свежести, сейчас, настройки, состояние.
    /// Возвращает: текст строки или None (не пора / частота не задана). Счётчики обнуляются. Ошибок нет.
    pub fn summary(&mut self, h: &FeedHealth, now: i64, c: &Cfg, st: &State) -> Option<String> {
        let every = self.every?;
        if self.summary_at.is_some_and(|t| now - t < every) {
            return None;
        }
        self.summary_at = Some(now);
        let dead_after = c.book_stale_ms * c.dead_mult;
        let mut n = [0usize; 9];
        for k in &h.conns {
            n[0] += 1;
            n[1] += usize::from(k.alive_ms.is_some_and(|a| now - a <= c.live_ms));
            for p in &k.pairs {
                let key = (k.conn.clone(), p.ticker.clone());
                if p.last_ms.is_some_and(|l| now - l <= dead_after) {
                    n[2] += 1;
                    continue;
                }
                n[3] += 1;
                n[4 + match st.checks.get(&key).and_then(|s| s.told.as_ref()) {
                    Some(Told::Quiet) => 0,
                    Some(Told::Unchanged) => 1,
                    Some(Told::Lost) => 2,
                    Some(Told::Failed(_) | Told::NoConfirm) => 3,
                    None => 4,
                }] += 1;
            }
        }
        let busy = st.pairs.values().filter(|s| s.busy()).count();
        let line = format!(
            "Сторож пар: соединений {} (живых {}), свежих пар {}, молчат {} — тихие подтверждены {}, книга не менялась с \
             переподписки {}, кадры потеряны {}, без подтверждения или проверка не удалась {}, ещё не решено {}; в кругах \
             переподписки (попытки или пауза) {busy}. С прошлой сводки: переподписок {}, ожили {}, переподключений {}",
            n[0],
            n[1],
            n[2],
            n[3],
            n[4],
            n[5],
            n[6],
            n[7],
            n[8],
            self.resubs,
            self.recovered,
            self.reconnects
        );
        (self.resubs, self.recovered, self.reconnects) = (0, 0, 0);
        Some(line)
    }
}

/// Режим подтверждения тихих пар площадки словами (решение 213). Ошибок нет.
fn mode(h: &FeedHealth, c: &Cfg) -> String {
    let base = match (&c.quiet, h.confirm) {
        (QuietCfg::Confirm { min_ms, max_per_s }, true) => format!(
            "тихие пары подтверждаются по REST (не чаще раза в {min_ms} мс на пару, до {max_per_s} запросов/с)"
        ),
        (QuietCfg::Off(why), true) => format!("подтверждение тихих пар по REST выключено: {why}"),
        (_, false) => "адаптер не подтверждает стакан по REST".into(),
    };
    if h.scheduled {
        format!("{base}; кадры по расписанию — тишина без подтверждения = мёртвая подписка")
    } else {
        base
    }
}
