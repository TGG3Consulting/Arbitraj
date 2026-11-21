//! Файл: venues/htx/book_fast.rs
//! Что делает: решение 422 — два метода стаканов HTX в памяти для выбора «быстрый стакан / снимок» (book_mode.rs):
//! запасной снимок кладётся со СВОИМ временем кадра (put_at — его свежесть по venue.book_stale_ms считается от
//! получения снимка, а не от переключения); сверка верхних уровней снимка со стаканом в памяти (same_top — без копий).
//! Кто вызывает: book_mode.rs (same_top), ws.rs и fast_pair.rs (put_at при уходе с быстрого стакана).
//! От чего зависит: book.rs (Books, Alive, Entry), core/types.
//! Настройки: нет.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use crate::core::types::{OrderBook, PriceLevel};

use super::book::{Alive, Books};

impl Books {
    /// Сохраняет стакан и связь с соединением; время кадра пары — seen_ms (unix мс), подтверждения тихой пары нет.
    /// Ошибок нет.
    pub fn put_at(&self, book: OrderBook, alive: Alive, seen_ms: i64) {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        let seen: Alive = Arc::new(AtomicI64::new(seen_ms));
        let until: Alive = Arc::new(AtomicI64::new(0));
        m.insert(book.asset.ticker.clone(), (book, alive, seen, until));
    }

    /// Совпадают ли первые n уровней каждой стороны стакана пары в памяти с данными (сторона короче n — сравнивается
    /// целиком). Принимает: тикер, уровни bids и asks от лучшей цены, n. Возвращает: false — стакана нет или другой.
    /// Ошибок нет.
    pub fn same_top(
        &self,
        ticker: &str,
        bids: &[PriceLevel],
        asks: &[PriceLevel],
        n: usize,
    ) -> bool {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        let Some((b, ..)) = m.get(ticker) else {
            return false;
        };
        let head = |s: &[PriceLevel]| s.len().min(n);
        b.bids[..head(&b.bids)] == bids[..head(bids)]
            && b.asks[..head(&b.asks)] == asks[..head(asks)]
    }
}
