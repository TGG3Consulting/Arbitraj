//! Файл: core/quote_cache.rs
//! Что делает: память котировок DEX для пересчёта на тике (решение 109). Тик биржи (кадр стакана) пересчитывает
//! направления «биржа ↔ DEX»; котировка DEX — обход пулов и графа маршрутов, самая дорогая операция бота, а пулы
//! между кадрами биржи обычно не меняются. Котировка DEX берётся из памяти, если: тот же объём, пулы этой монеты у
//! этой площадки не менялись (поколение растёт на каждом событии пула и снятии пулов — tick.rs) и котировке не больше
//! signal.dex_quote_cache_ms. Не задано или 0 — памяти нет (котировка каждый раз). На каждую (площадку, монету,
//! сторону) хранится одна последняя котировка — память ограничена числом монет.
//! Только пересчёт на тике (prices.rs): полный расчёт кандидата и сделка котируют заново всегда.
//! Кто вызывает: core/prices.rs (get, put), core/tick.rs (bump).
//! От чего зависит: core/types.
//! Настройки: signal.dex_quote_cache_ms (читает prices.rs).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Quote, VenueId};

/// Ключ: площадка, тикер, покупка (true) / продажа.
type Key = (VenueId, String, bool);

/// Память котировок DEX и поколения пулов.
#[derive(Default)]
pub struct QuoteCache {
    quotes: HashMap<Key, (Decimal, Quote, Instant, u64)>,
    gens: HashMap<(VenueId, Address), u64>,
}

impl QuoteCache {
    /// Пулы токена у площадки изменились — котировки с ним больше не годятся. Ошибок нет.
    pub fn bump(&mut self, venue: &VenueId, token: Address) {
        *self.gens.entry((venue.clone(), token)).or_default() += 1;
    }

    /// Поколение пулов токена у площадки (0 — событий ещё не было). Ошибок нет.
    pub fn generation(&self, venue: &VenueId, token: Address) -> u64 {
        self.gens.get(&(venue.clone(), token)).copied().unwrap_or(0)
    }

    /// Котировка из памяти: тот же объём, то же поколение пулов, не старше ttl. None — нет годной.
    pub fn get(&self, key: &Key, qty: Decimal, generation: u64, ttl: Duration) -> Option<Quote> {
        let (q_qty, quote, at, g) = self.quotes.get(key)?;
        (*q_qty == qty && *g == generation && at.elapsed() <= ttl).then(|| quote.clone())
    }

    /// Запоминает котировку (заменяет прежнюю для той же площадки, монеты и стороны). Ошибок нет.
    pub fn put(&mut self, key: Key, qty: Decimal, quote: Quote, generation: u64) {
        self.quotes
            .insert(key, (qty, quote, Instant::now(), generation));
    }
}
