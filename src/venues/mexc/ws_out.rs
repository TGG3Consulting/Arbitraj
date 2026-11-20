//! Файл: venues/mexc/ws_out.rs
//! Что делает: отдача стаканов одного соединения MEXC ядру — вынесена из ws_conn.rs (решение 420), логика прежняя:
//! стакан в память (book.rs put) и ядру (gateway/core_out_book.rs, решение 324); тот же стакан, что отдан прошлым
//! (решение 109), — ядру не отдаётся, только время кадра пары (book.rs touch; стакан из ответа REST сторожа — t79 —
//! кладётся заново). Решение 420: при быстром стакане (strict) стакан с временем биржи (sendTime) меньше уже отданного
//! ядру не отдаётся — ни кадр потока снимков, ни быстрый стакан («никогда не отдавать ядру стакан старее уже отданного»,
//! как решение 109); без быстрого стакана — как раньше, без этой проверки. Последний кадр снимков, отданный при
//! переключении на страховку (fast_pair.rs), кладётся со временем кадра пары = временем его получения (give_seen), а
//! не «сейчас»: свежесть по venue.book_stale_ms считается от кадра, как всегда.
//! Кто вызывает: ws_conn.rs, fast_pair.rs (через ws_conn.rs).
//! От чего зависит: book.rs, depth.rs (to_book), gateway/core_out_book.rs.
//! Настройки: venue.book_events_keep (через core_out_book.rs).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;

use super::book::{Alive, Books};
use super::ctx::Ctx;
use super::depth;

/// Итог отдачи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gave {
    /// Ушёл ядру (или ждёт в наборе — решение 324).
    Sent,
    /// Канал полон — потерян (счёт dropped).
    Lost,
    /// Тот же стакан, что прошлый, — только время кадра пары.
    Same,
    /// Время биржи меньше уже отданного — не отдан (strict).
    Older,
    /// Канал ядра закрыт.
    Closed,
}

/// Отдача стаканов соединения.
pub struct Out<'a> {
    kept: BookOut,
    out: &'a mpsc::Sender<MarketEvent>,
    books: &'a Books,
    alive: Alive,
    /// Последний отданный ядру стакан по символу (решение 109).
    last: HashMap<String, (Vec<PriceLevel>, Vec<PriceLevel>)>,
    /// Время биржи последнего отданного ядру стакана по символу (решение 420), unix мс.
    last_ms: HashMap<String, i64>,
    /// Решение 420: проверка «не старее отданного» (только при быстром стакане).
    strict: bool,
    /// Потеряно стаканов (канал полон).
    pub dropped: u64,
}

impl<'a> Out<'a> {
    /// Что делает: отдача для сессии. Принимает: окружение, канал ядра, стаканы, живость сессии, strict. Ошибок нет.
    pub fn new(
        ctx: &Ctx,
        out: &'a mpsc::Sender<MarketEvent>,
        books: &'a Books,
        alive: Alive,
        strict: bool,
    ) -> Self {
        Self {
            kept: BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal),
            out,
            books,
            alive,
            last: HashMap::new(),
            last_ms: HashMap::new(),
            strict,
            dropped: 0,
        }
    }

    /// Что делает: стакан пары ядру (правила — шапка). Принимает: монету, символ MEXC, уровни, время биржи.
    /// Возвращает: Gave. Ошибок нет.
    pub fn give(
        &mut self,
        asset: &Asset,
        symbol: &str,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        time: Option<DateTime<Utc>>,
    ) -> Gave {
        self.give_seen(asset, symbol, (bids, asks), time, None)
    }

    /// Что делает: как give, но время кадра пары — seen_ms (unix мс, часы бота; None — сейчас). Ошибок нет.
    pub fn give_seen(
        &mut self,
        asset: &Asset,
        symbol: &str,
        (bids, asks): (Vec<PriceLevel>, Vec<PriceLevel>),
        time: Option<DateTime<Utc>>,
        seen_ms: Option<i64>,
    ) -> Gave {
        let ms = time.map(|t| t.timestamp_millis());
        if self.strict
            && let (Some(t), Some(prev)) = (ms, self.last_ms.get(symbol))
            && t < *prev
        {
            return Gave::Older;
        }
        // Время кадра пары задано (кадр снимков при переключении) — не продлевать до «сейчас», а положить с ним.
        if seen_ms.is_none()
            && self
                .last
                .get(symbol)
                .is_some_and(|(b, a)| *b == bids && *a == asks)
            && self.books.touch(&asset.ticker)
        {
            return Gave::Same;
        }
        self.last
            .insert(symbol.to_string(), (bids.clone(), asks.clone()));
        if let Some(t) = ms {
            self.last_ms.insert(symbol.to_string(), t);
        }
        let book = depth::to_book(asset.clone(), bids, asks, time);
        match seen_ms {
            Some(ms) => self.books.put_at(book.clone(), self.alive.clone(), ms),
            None => self.books.put(book.clone(), self.alive.clone()),
        }
        match self.kept.book(self.out, book) {
            Fed::Sent | Fed::Kept => Gave::Sent,
            Fed::Closed => Gave::Closed,
            Fed::Lost => {
                self.dropped += 1;
                Gave::Lost
            }
        }
    }

    /// Что делает: следующий стакан пары — ядру, даже если такой же (переподписка, стакан снят). Принимает: символ;
    /// None — все пары. Ошибок нет.
    pub fn forget(&mut self, symbol: Option<&str>) {
        match symbol {
            Some(s) => {
                self.last.remove(s);
            }
            None => self.last.clear(),
        }
    }
}

impl Books {
    /// Решение 420: как put, но время кадра пары — seen_ms (unix мс). Ошибок нет.
    pub(super) fn put_at(&self, book: OrderBook, alive: Alive, seen_ms: i64) {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        let seen = Arc::new(AtomicI64::new(seen_ms));
        let until = Arc::new(AtomicI64::new(0));
        m.insert(book.asset.ticker.clone(), (book, alive, seen, until, false));
    }
}
