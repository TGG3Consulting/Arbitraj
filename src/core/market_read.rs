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
        let mut gone = Vec::new();
        self.pools.retain(|(v, r), p| {
            let drop = v == venue && (p.token0 == token || p.token1 == token);
            if drop {
                gone.push((
                    super::market_state::pair_key(v, p.token0, p.token1),
                    r.clone(),
                ));
            }
            !drop
        });
        for (k, r) in gone {
            if let Some(s) = self.pool_pairs.get_mut(&k) {
                s.remove(&r);
                if s.is_empty() {
                    self.pool_pairs.remove(&k);
                }
            }
        }
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

    /// Решение 227: тихая пара подтверждена по REST (решение 213, feed_watch_act.rs) — стакан пары считается свежим для
    /// худшей цены ордера (worst.rs) до until_ms (unix мс), как в котировке адаптера. Ошибок нет.
    pub fn set_quiet(&mut self, venue: &VenueId, ticker: &str, until_ms: i64) {
        self.quiet
            .insert((venue.clone(), ticker.to_string()), until_ms);
    }

    /// Решение 227: до какого момента (unix мс) стакан тихой пары подтверждён. None — подтверждения не было.
    pub fn quiet_until(&self, venue: &VenueId, ticker: &str) -> Option<i64> {
        self.quiet
            .get(&(venue.clone(), ticker.to_string()))
            .copied()
    }

    /// Пулы пары токенов на площадке (монета с USDT или с промежуточной монетой, решение 52).
    /// Принимает: площадку, адрес монеты, адрес второго токена.
    /// Решение 129: по индексу пары (market_state.rs pool_pairs), а не проходом всех пулов.
    pub fn usdt_pools(&self, venue: &VenueId, token: Address, usdt: Address) -> Vec<&Pool> {
        let Some(refs) = self
            .pool_pairs
            .get(&super::market_state::pair_key(venue, token, usdt))
        else {
            return Vec::new();
        };
        refs.iter()
            .filter_map(|r| self.pools.get(&(venue.clone(), r.clone())))
            .collect()
    }

    /// Снимок для сборки справочника монет (решение 129): всё, что читают registry_build.rs, presence.rs,
    /// subscribe.rs (cex_pair), pairs_fresh.rs и rebuild_blacklist.rs, — чтобы сборка шла без блокировки рынка (тик
    /// пишет в рынок на каждое событие). Стаканы — без уровней (справочнику нужно только «стакан пришёл»); каналы и
    /// очереди кошелька не копируются. Ошибок нет.
    pub fn registry_view(&self) -> MarketState {
        MarketState {
            books: self
                .books
                .iter()
                .map(|(k, b)| {
                    let head = OrderBook {
                        venue: b.venue.clone(),
                        asset: b.asset.clone(),
                        bids: vec![],
                        asks: vec![],
                        exchange_time: b.exchange_time,
                        received_at: b.received_at,
                    };
                    (k.clone(), head)
                })
                .collect(),
            pools: self.pools.clone(),
            pool_pairs: self.pool_pairs.clone(),
            coin_checks: self.coin_checks.clone(),
            pool_checks: self.pool_checks.clone(),
            spot: self.spot.clone(),
            loading: self.loading.clone(),
            pairs: self.pairs.clone(),
            pairs_at: self.pairs_at.clone(),
            ..Default::default()
        }
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
