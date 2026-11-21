//! Файл: core/quote_cache.rs
//! Что делает: память котировок DEX для пересчёта на тике (решение 109). Тик биржи (кадр стакана) пересчитывает
//! направления «биржа ↔ DEX»; котировка DEX — обход пулов и графа маршрутов, самая дорогая операция бота, а пулы
//! между кадрами биржи обычно не меняются. Котировка DEX берётся из памяти, если: тот же объём, пулы этой монеты у
//! этой площадки не менялись (поколение растёт на каждом событии пула и снятии пулов — tick.rs) и котировке не больше
//! signal.dex_quote_cache_ms. Не задано или 0 — памяти нет (котировка каждый раз).
//! Решение 118 (Я-7): ключ — (площадка, адрес токена, сторона), а не тикер; на ключ — маленькая карта объём →
//! котировка: продажа на DEX нужна BingX → DEX и MEXC → DEX на разных объёмах, и они больше не затирают друг друга.
//! При записи удаляются котировки старше срока и прошлых поколений — память ограничена числом монет × направлений.
//! Только пересчёт на тике (prices.rs): полный расчёт кандидата и сделка котируют заново всегда.
//! Кто вызывает: core/prices.rs (get, put), core/tick.rs (bump).
//! От чего зависит: core/types.
//! Настройки: signal.dex_quote_cache_ms (читает prices.rs).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Quote, VenueId};

/// Ключ: площадка, адрес токена, покупка (true) / продажа.
pub type Key = (VenueId, Address, bool);

/// Память котировок DEX и поколения пулов.
#[derive(Default)]
pub struct QuoteCache {
    /// Ключ → объём → (котировка, когда, поколение пулов).
    quotes: HashMap<Key, HashMap<Decimal, (Quote, Instant, u64)>>,
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
        let (quote, at, g) = self.quotes.get(key)?.get(&qty)?;
        (*g == generation && at.elapsed() <= ttl).then(|| quote.clone())
    }

    /// Запоминает котировку на объём (заменяет прежнюю на тот же объём); котировки ключа старше ttl или прошлых
    /// поколений удаляются. Принимает: ключ, объём, котировку, поколение, срок. Ошибок нет.
    pub fn put(&mut self, key: Key, qty: Decimal, quote: Quote, generation: u64, ttl: Duration) {
        let m = self.quotes.entry(key).or_default();
        m.retain(|_, (_, at, g)| *g == generation && at.elapsed() <= ttl);
        m.insert(qty, (quote, Instant::now(), generation));
    }
}
