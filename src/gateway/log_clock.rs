//! Файл: gateway/log_clock.rs
//! Что делает: часы записи журнала «не чаще интервала» (чистая логика, без окружения): первая запись — сразу, внутри
//! прочитанного интервала — нет и настройка не читается, интервал не задан — записей нет. Перенесено из
//! venues/bsc_dex/pools/lost_events.rs (решение 313) в шлюз решением 324 — те же часы у записи стаканов бирж
//! (gateway/book_lost_log.rs); lost_events.rs реэкспортирует Clock.
//! Кто вызывает: venues/bsc_dex/pools/lost_events.rs, gateway/book_lost_log.rs.
//! От чего зависит: std::time.
//! Настроек нет (интервал читает вызывающий).

use std::time::{Duration, Instant};

/// Часы записи (чистая логика, без окружения): прошлая запись и прочитанный при ней интервал.
#[derive(Debug, Default)]
pub struct Clock {
    /// Когда была прошлая запись (None — ещё не было).
    last: Option<Instant>,
    /// Интервал, прочитанный в прошлую проверку (None — не задан или ещё не читался).
    every: Option<Duration>,
}

impl Clock {
    /// Пора ли писать. Принимает: «сейчас», чтение интервала (вызывается, только если без него не решить: интервал не
    /// прочитан / не задан или прочитанный уже прошёл). Возвращает: None — не писать (не задан или рано);
    /// Some(с прошлой записи; None — первая запись). Ошибок нет.
    pub fn due(
        &mut self,
        now: Instant,
        read: impl FnOnce() -> Option<Duration>,
    ) -> Option<Option<Duration>> {
        if let (Some(t), Some(e)) = (self.last, self.every)
            && now.duration_since(t) < e
        {
            return None;
        }
        let Some(every) = read() else {
            self.every = None;
            return None;
        };
        self.every = Some(every);
        if let Some(t) = self.last
            && now.duration_since(t) < every
        {
            return None;
        }
        let since = self.last.map(|t| now.duration_since(t));
        self.last = Some(now);
        Some(since)
    }
}
