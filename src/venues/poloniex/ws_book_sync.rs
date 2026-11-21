//! Файл: venues/poloniex/ws_book_sync.rs
//! Что делает: решение 424 — пересинхронизация быстрого стакана Poloniex и сводка в журнал (часть BookConn из
//! ws_book.rs, вынесена ради размера файла): переподписка book_lv2 пары после разрыва (отписка, затем подписка — в
//! общую очередь отправки соединения), не чаще venue.ws_reconnect_pause_ms на пару (урок 12: без круга ресинков —
//! раньше срока переподписка откладывается); сводка переключений «на быстрый / на снимки / разрывов / переподписок» —
//! запись журнала venue.book_fast не чаще log.pool_events_lost_interval_s (gateway/log_clock.rs; первая — сразу; не
//! задана — записей нет, счёт копится в памяти — несколько чисел).
//! Кто вызывает: ws_book.rs (resync), ws_conn.rs (next_due, release, note).
//! От чего зависит: ws_book.rs (BookConn), fast_book.rs (Stats), lv2.rs (сообщения), gateway/{core_out_book (имя
//! настройки), log_clock}.rs, storage/journal.rs.
//! Настройки: venue.ws_reconnect_pause_ms (через ws.rs Cfg), log.pool_events_lost_interval_s [вся система / площадка].
//!
//! Официально (Market Data → Book Level 2, https://api-docs.poloniex.com/spot/websocket/market-data): при несовпадении
//! lastId — «must re-subscribe to the channel»; отписка — Spot → WebSocket, «Unsubscribe». Новое тяжёлое логирование
//! не заводится: одна строка за интервал на соединение.

use std::time::{Duration, Instant};

use crate::gateway::core_out_book::INTERVAL;
use crate::storage::journal::Level;

use super::fast_book::Stats;
use super::lv2;
use super::ws_book::BookConn;

impl BookConn<'_> {
    /// Ближайшая отложенная переподписка. Ошибок нет.
    pub fn next_due(&self) -> Option<Instant> {
        self.deferred.values().min().copied()
    }

    /// Отложенные переподписки, чей срок пришёл, — в очередь. Ошибок нет.
    pub fn release(&mut self, pending: &mut Vec<String>) {
        let now = Instant::now();
        let due: Vec<String> = self
            .deferred
            .iter()
            .filter(|(_, t)| **t <= now)
            .map(|(s, _)| s.clone())
            .collect();
        for s in due {
            self.deferred.remove(&s);
            self.send_resync(&s, now, pending);
        }
    }

    /// Переподписка book_lv2 пары: сразу или, если прошлая была раньше venue.ws_reconnect_pause_ms, — отложенно.
    /// Пауза не задана — переподписки нет (пара остаётся на снимках book). Ошибок нет.
    pub(super) fn resync(&mut self, symbol: &str, pending: &mut Vec<String>) {
        if self.resync_pause.is_none() || self.deferred.contains_key(symbol) {
            return;
        }
        let now = Instant::now();
        match self.not_before.get(symbol).copied() {
            Some(t) if t > now => {
                self.deferred.insert(symbol.to_string(), t);
            }
            _ => self.send_resync(symbol, now, pending),
        }
    }

    /// Отписка и подписка book_lv2 пары в очередь (очередь берёт с конца: сначала отписка). Ошибок нет.
    fn send_resync(&mut self, symbol: &str, now: Instant, pending: &mut Vec<String>) {
        let Some(pause) = self.resync_pause else {
            return;
        };
        pending.push(lv2::sub_message(symbol));
        pending.push(lv2::unsub_message(symbol));
        self.not_before.insert(symbol.to_string(), now + pause);
        self.fast.stats.resyncs += 1;
    }

    /// Сводка переключений в журнал venue.book_fast, если есть что сказать и пора (log.pool_events_lost_interval_s;
    /// первая — сразу). Запись — отдельной задачей, соединение не ждёт. Ошибок нет.
    pub fn note(&mut self) {
        if self.fast.stats == Stats::default() {
            return;
        }
        let ctx = self.ctx;
        let read = || {
            ctx.int(INTERVAL)
                .ok()
                .filter(|s| *s > 0)
                .map(Duration::from_secs)
        };
        if self.clock.due(Instant::now(), read).is_none() {
            return;
        }
        let s = std::mem::take(&mut self.fast.stats);
        let level = if s.gaps > 0 { Level::Warn } else { Level::Info };
        let text = format!(
            "WebSocket {}: стакан book_lv2 (решение 424) — на быстрый {}, на снимки book {}, разрывов {}, переподписок {}{}",
            self.name,
            s.to_fast,
            s.to_snap,
            s.gaps,
            s.resyncs,
            if s.last_why.is_empty() {
                String::new()
            } else {
                format!("; последний разрыв: {}", s.last_why)
            }
        );
        let c = ctx.clone();
        tokio::spawn(async move { c.log(level, "venue.book_fast", text).await });
    }
}
