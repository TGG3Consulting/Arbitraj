//! Файл: venues/kraken/standby.rs
//! Что делает: решение 426 — страховка стакана Kraken в памяти. Пары одного соединения стаканов слушаются ДВАЖДЫ:
//! основное соединение (глубина venue.depth_levels) и соединение-страховка (глубина venue.book_standby_depth) — каждое со
//! своим локальным стаканом и своей проверкой чек-суммы Kraken на каждом кадре (local_book.rs). Здесь — выбор, чей
//! стакан отдавать ядру (по паре): основной, пока он сверен; основной потерял стакан (чек-сумма, изменение до снимка,
//! отказ подписки, обрыв соединения) — сразу последний сверенный стакан страховки, если он не старше уже отданного;
//! основной снова сверен и его кадр не старше отданного — снова основной. Кадр старше уже отданного ядру не уходит
//! (как решение 109). Обе стороны недостоверны — стакан недостоверен (как раньше без страховки).
//! «Не старше»: время книги timestamp кадра больше отданного, или то же время и та же чек-сумма Kraken (то же
//! состояние верхних 10 уровней). Время у разных кадров бывает одинаковым (пример документации: снимок и изменение
//! MATIC/USD — оба 2023-10-06T17:35:55.440295Z), поэтому одного времени мало. Времени нет — переключения нет.
//! Отдача (запись в память адаптера и канал ядра) — внутри замка пары: кадры двух соединений не обгоняют друг друга.
//! Кто вызывает: ws_conn.rs (кадр сверен / пара потеряна), ws.rs (обрыв соединения), тесты.
//! От чего зависит: core/types (OrderBook), book.rs (Alive), gateway/feed_reg.rs (учёт страховки вне сторожа ядра).
//! Настройки: нет (глубину страховки читает ws.rs).
//!
//! Официально: https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md — data[].checksum «CRC32
//! checksum for the top 10 bids and asks», data[].timestamp — время книги (RFC3339), «Updates should always be
//! processed in sequence» (порядок — только внутри одной подписки, между соединениями порядка нет);
//! https://docs.kraken.com/exchange/guides/websockets/reconnection.md — при сбое чек-суммы «unsubscribe and
//! re-subscribe to force a fresh snapshot» (основной так и делает, страховка закрывает время до нового снимка).

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::core::types::OrderBook;
use crate::gateway::feed_reg::FeedReg;

use super::book::Alive;

/// Чьё соединение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Основное (venue.depth_levels) — его стакан отдаётся, пока он сверен.
    Main,
    /// Страховка (venue.book_standby_depth).
    Standby,
}

/// Версия стакана для сравнения «не старше»: время книги Kraken и чек-сумма верхних 10 уровней.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ver {
    pub ts: Option<DateTime<Utc>>,
    pub crc: u32,
}

/// Сверенный кадр: стакан, живость его соединения, время получения кадра (unix мс), версия.
#[derive(Clone)]
pub struct Frame {
    pub book: OrderBook,
    pub alive: Alive,
    pub seen_ms: i64,
    pub ver: Ver,
}

/// Переключение источника пары.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    ToStandby,
    ToMain,
}

/// Чей стакан сейчас у ядра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Active {
    /// Основной ещё не отдавал (страховка до первого кадра основного не отдаётся — как без страховки).
    Start,
    Main,
    Standby,
    /// Обе стороны недостоверны — ядру отдано «недостоверен».
    Lost,
}

/// Состояние пары.
struct Pair {
    active: Active,
    /// Версия последнего отданного ядру.
    last: Option<Ver>,
    /// Последний сверенный кадр страховки (None — страховка не сверена).
    standby: Option<Frame>,
}

/// Выбор стакана пар одного соединения (основное + страховка). Создаётся на подписку (market.rs).
#[derive(Default)]
pub struct Arbiter {
    pairs: Mutex<HashMap<String, Pair>>,
    /// Учёт свежести соединения-страховки: не виден сторожу ядра (иначе удвоились бы его проверки REST тихих пар);
    /// целостность страховки — чек-сумма, живость — её venue.ws_silence_ms.
    pub feed: FeedReg,
}

/// Кадр не старше отданного: время больше, или то же время и та же чек-сумма. Отданного нет — да; времени нет — нет.
fn not_older(c: &Ver, last: &Option<Ver>) -> bool {
    match last {
        None => true,
        Some(l) => match (c.ts, l.ts) {
            (Some(a), Some(b)) => a > b || (a == b && c.crc == l.crc),
            _ => false,
        },
    }
}

/// Состояние пары (новая — Start). Строка тикера создаётся только для новой пары. Ошибок нет.
fn slot<'a>(m: &'a mut HashMap<String, Pair>, ticker: &str) -> &'a mut Pair {
    if !m.contains_key(ticker) {
        let p = Pair {
            active: Active::Start,
            last: None,
            standby: None,
        };
        m.insert(ticker.to_string(), p);
    }
    m.get_mut(ticker)
        .unwrap_or_else(|| unreachable!("пара вставлена выше"))
}

impl Arbiter {
    /// Что делает: сверенный кадр пары от соединения role. Отдаёт (emit) под замком, если этот источник сейчас
    /// активен или переключение на него допустимо (см. шапку). Принимает: роль, тикер, кадр, отдачу.
    /// Возвращает: (итог отдачи — None, кадр не отдан; переключение). Ошибок нет.
    pub fn offer<R>(
        &self,
        role: Role,
        ticker: &str,
        f: Frame,
        emit: impl FnOnce(Frame) -> R,
    ) -> (Option<R>, Option<Switch>) {
        let mut m = self.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let p = slot(&mut m, ticker);
        let ver = f.ver;
        let (go, switch) = match (role, p.active) {
            (Role::Main, Active::Start | Active::Main) => (true, None),
            (Role::Main, _) => (not_older(&ver, &p.last), Some(Switch::ToMain)),
            (Role::Standby, Active::Standby) => (true, None),
            (Role::Standby, Active::Lost) => (not_older(&ver, &p.last), Some(Switch::ToStandby)),
            (Role::Standby, _) => (false, None),
        };
        if !go {
            if role == Role::Standby {
                p.standby = Some(f);
            }
            return (None, None);
        }
        if role == Role::Standby {
            p.standby = Some(f.clone());
        }
        p.active = if role == Role::Main {
            Active::Main
        } else {
            Active::Standby
        };
        p.last = Some(ver);
        (Some(emit(f)), switch)
    }

    /// Что делает: соединение role потеряло стакан пары (до нового снимка). Основной был активен — стакан страховки,
    /// если он сверен и не старше отданного (emit), иначе «недостоверен» (lost); активна страховка и потеряна она —
    /// «недостоверен». Неактивная сторона — только забывается. Принимает: роль, тикер, отдачу, «недостоверен».
    /// Возвращает: (итог отдачи, переключение, отдано ли «недостоверен»). Ошибок нет.
    pub fn lost<R>(
        &self,
        role: Role,
        ticker: &str,
        emit: impl FnOnce(Frame) -> R,
        lost: impl FnOnce(),
    ) -> (Option<R>, Option<Switch>, bool) {
        let mut m = self.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let p = slot(&mut m, ticker);
        if role == Role::Standby {
            p.standby = None;
        }
        match (role, p.active) {
            (Role::Main, Active::Main) => {
                if let Some(s) = p.standby.as_ref().filter(|s| not_older(&s.ver, &p.last)) {
                    let s = s.clone();
                    p.active = Active::Standby;
                    p.last = Some(s.ver);
                    return (Some(emit(s)), Some(Switch::ToStandby), false);
                }
                p.active = Active::Lost;
                lost();
                (None, None, true)
            }
            // Основной ещё ничего не отдавал — «недостоверен», как без страховки.
            (Role::Main, Active::Start) | (Role::Standby, Active::Standby) => {
                if p.active == Active::Standby {
                    p.active = Active::Lost;
                }
                lost();
                (None, None, true)
            }
            _ => (None, None, false),
        }
    }
}
