//! Файл: core/market_apply.rs
//! Что делает: применение стакана биржи и состояния пула DEX к состоянию рынка ядра (market_state.rs apply). Стакан —
//! замена по (площадка, тикер), отметки изменения содержимого (решение 317, book_marks_on.rs) при включённом признаке.
//! Пул — замена по (площадка, ссылка); первый пул пары — в индекс пар (решение 129).
//! Кто вызывает: core/market_state.rs (apply).
//! От чего зависит: market_state.rs (поля, pair_key), book_marks_on.rs, core/types.
//! Настройки: нет.

use crate::core::types::{OrderBook, Pool};

use super::market_state::{MarketState, Touched, pair_key};

impl MarketState {
    /// Что делает: новый стакан. Принимает: стакан. Возвращает: Touched::Book (first — стакан пары впервые). Ошибок нет.
    pub(super) fn apply_book(&mut self, b: OrderBook) -> Touched {
        let key = (b.venue.clone(), b.asset.ticker.clone());
        // Один поиск стакана (get_mut + замена; нет — вставка). Отметки — только при включённом признаке.
        match self.books.get_mut(&key) {
            Some(slot) => {
                if self.marks_on {
                    super::book_marks_on::note(
                        &mut self.book_marks,
                        &key,
                        Some(&*slot),
                        &b,
                        self.book_gap_ms.get(&b.venue).copied(),
                    );
                }
                *slot = b;
                let (venue, ticker) = key;
                Touched::Book {
                    venue,
                    ticker,
                    first: false,
                }
            }
            None => {
                if self.marks_on {
                    super::book_marks_on::note(&mut self.book_marks, &key, None, &b, None);
                }
                let (venue, ticker) = key.clone();
                self.books.insert(key, b);
                Touched::Book {
                    venue,
                    ticker,
                    first: true,
                }
            }
        }
    }

    /// Что делает: новое состояние пула. Принимает: пул. Возвращает: Touched::Pool (first — пул впервые). Ошибок нет.
    pub(super) fn apply_pool(&mut self, p: Pool) -> Touched {
        let key = (p.venue.clone(), p.reference.clone());
        let (venue, tokens) = (p.venue.clone(), (p.token0, p.token1));
        let r = p.reference.clone();
        let block = p.block;
        let first = self.pools.insert(key, p).is_none();
        if first {
            self.pool_pairs
                .entry(pair_key(&venue, tokens.0, tokens.1))
                .or_default()
                .insert(r.clone());
        }
        Touched::Pool {
            venue,
            tokens,
            pool: r,
            first,
            block,
        }
    }
}
