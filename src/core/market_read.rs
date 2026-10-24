//! Файл: core/market_read.rs
//! Что делает: чтение рыночного состояния ядра (market_state.rs): пулы «загружаются», пары и их свежесть, стаканы и
//! лучшие цены, пулы с USDT, проверки монет и пулов, каналы отправки; удаление пулов токена.
//! Кто вызывает: registry_build.rs, presence.rs, signal.rs, prices.rs, pairs_fresh.rs, subscribe.rs и др. ядра.
//! От чего зависит: market_state.rs (MarketState), core/types, gateway/events.
//! Настройки: нет.

use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{
    CoinCheck, OrderBook, Pool, PoolCheck, PoolRef, SendChannelStats, Side, VenueId,
};
use crate::gateway::events::SpotPairState;

use super::market_state::MarketState;

impl MarketState {
    /// Пулы монеты на DEX найдены, но ещё не прочитаны (решение 61). Ошибок нет.
    pub fn pools_loading(&self, venue: &VenueId, token: Address) -> bool {
        self.loading.get(venue).is_some_and(|s| s.contains(&token))
    }

    /// Удаляет пулы токена на площадке (DEX объявил данные монеты недостоверными).
    /// Принимает: площадку, адрес токена. Возвращает: сколько пулов удалено.
    pub fn drop_pools_of(&mut self, venue: &VenueId, token: Address) -> usize {
        let before = self.pools.len();
        self.pools
            .retain(|(v, _), p| !(v == venue && (p.token0 == token || p.token1 == token)));
        before - self.pools.len()
    }

    /// Есть ли у монеты торгуемая спот-пара на бирже — по последнему SpotPairs этой биржи (до него — нет).
    /// Не зависит от стаканов: разрыв связи и «недостоверно» список не меняют. Ошибок нет.
    pub fn spot_pair(&self, venue: &VenueId, ticker: &str) -> bool {
        self.spot.get(venue).is_some_and(|s| s.contains(ticker))
    }

    /// Состояние спот-пары монеты на бирже по последнему SpotPairs (core/presence.rs). None — биржа ещё не прислала
    /// SpotPairs или пары у монеты в справочнике биржи нет. Ошибок нет.
    pub fn pair(&self, venue: &VenueId, ticker: &str) -> Option<&SpotPairState> {
        self.pairs.get(venue).and_then(|m| m.get(ticker))
    }

    /// Когда справочник пар биржи последний раз получен от неё. None — SpotPairs ещё не было. Ошибок нет.
    pub fn pairs_loaded_at(&self, venue: &VenueId) -> Option<DateTime<Utc>> {
        self.pairs_at.get(venue).copied()
    }

    /// Стакан монеты на площадке. None — стакана нет (не пришёл или объявлен недостоверным).
    pub fn book(&self, venue: &VenueId, ticker: &str) -> Option<&OrderBook> {
        self.books.get(&(venue.clone(), ticker.to_string()))
    }

    /// Лучшая цена стакана: для покупки — лучший ask, для продажи — лучший bid. None — нет уровней.
    pub fn best(&self, venue: &VenueId, ticker: &str, side: Side) -> Option<Decimal> {
        let b = self.book(venue, ticker)?;
        let levels = match side {
            Side::Buy => &b.asks,
            Side::Sell => &b.bids,
        };
        levels.first().map(|l| l.price)
    }

    /// Цена самого глубокого уровня, до которого дойдёт объём qty (худшая цена лимитного ордера,
    /// чтобы исполнить весь объём по текущему стакану). None — нет стакана или глубины не хватает.
    pub fn deepest_price(
        &self,
        venue: &VenueId,
        ticker: &str,
        side: Side,
        qty: Decimal,
    ) -> Option<Decimal> {
        let b = self.book(venue, ticker)?;
        let levels = match side {
            Side::Buy => &b.asks,
            Side::Sell => &b.bids,
        };
        let mut left = qty;
        for l in levels {
            left -= l.qty;
            if left <= Decimal::ZERO {
                return Some(l.price);
            }
        }
        None
    }

    /// Пулы пары токенов на площадке (монета с USDT или с промежуточной монетой, решение 52).
    /// Принимает: площадку, адрес монеты, адрес второго токена.
    pub fn usdt_pools(&self, venue: &VenueId, token: Address, usdt: Address) -> Vec<&Pool> {
        self.pools
            .iter()
            .filter(|((v, _), p)| {
                v == venue
                    && ((p.token0 == token && p.token1 == usdt)
                        || (p.token1 == token && p.token0 == usdt))
            })
            .map(|(_, p)| p)
            .collect()
    }

    /// Последняя проверка налогов монеты на площадке DEX. None — проверок ещё не было.
    pub fn coin_check(&self, venue: &VenueId, token: Address) -> Option<&CoinCheck> {
        self.coin_checks.get(&(venue.clone(), token))
    }

    /// Последняя проверка крючка пула. None — проверок не было.
    pub fn pool_check(&self, venue: &VenueId, pool: &PoolRef) -> Option<&PoolCheck> {
        self.pool_checks.get(&(venue.clone(), pool.clone()))
    }

    /// Последнее состояние всех каналов отправки (для экрана «Каналы отправки»).
    pub fn channels(&self) -> Vec<SendChannelStats> {
        self.channels.values().cloned().collect()
    }
}
