//! Файл: core/market_state.rs
//! Что делает: всё, что ядро знает о рынке прямо сейчас (шаг 2 ТЗ): стаканы бирж, пулы DEX,
//! проверки налогов монет и крючков пулов, состояние каналов отправки — из событий адаптеров.
//! Данные, которые площадка объявила недостоверными, удаляются: по ним сделок нет.
//! Кто вызывает: core/engine.rs (применяет события), registry.rs, signal.rs, prices.rs, позже executor.rs.
//! От чего зависит: core/types, gateway/events.
//! Настройки: нет. Свежесть цены проверяет адаптер при каждой котировке (feed.stale_after_ms):
//! только он знает, живо ли соединение (BingX шлёт стакан лишь при изменении, см. venues/bingx/book.rs).
//! Ядро не торгует без свежей котировки — значит, не торгует по устаревшим данным.

use std::collections::HashMap;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{
    CoinCheck, OrderBook, Pool, PoolCheck, PoolRef, SendChannelStats, Side, VenueId,
    WalletQueueState,
};
use crate::gateway::events::MarketEvent;

pub use super::touched::Touched;

/// Рыночное состояние в памяти.
#[derive(Default)]
pub struct MarketState {
    books: HashMap<(VenueId, String), OrderBook>,
    pools: HashMap<(VenueId, PoolRef), Pool>,
    /// Проверка налогов: ключ — площадка DEX и адрес контракта монеты.
    coin_checks: HashMap<(VenueId, Address), CoinCheck>,
    pool_checks: HashMap<(VenueId, PoolRef), PoolCheck>,
    /// Каналы отправки по id (в SendChannelStats площадки нет; каналы есть только у адаптера DEX).
    channels: HashMap<String, SendChannelStats>,
    /// Последнее состояние очереди кошелька по площадке DEX — уведомления только на смену состояния.
    queues: HashMap<VenueId, WalletQueueState>,
}

impl MarketState {
    /// Применяет событие адаптера. Принимает: событие. Возвращает: что изменилось. Ошибок нет.
    pub fn apply(&mut self, ev: MarketEvent) -> Touched {
        match ev {
            MarketEvent::Book(b) => {
                let key = (b.venue.clone(), b.asset.ticker.clone());
                let (venue, ticker) = key.clone();
                let first = self.books.insert(key, b).is_none();
                Touched::Book {
                    venue,
                    ticker,
                    first,
                }
            }
            MarketEvent::Pool(p) => {
                let key = (p.venue.clone(), p.reference.clone());
                let (venue, tokens) = (p.venue.clone(), (p.token0, p.token1));
                let first = self.pools.insert(key, p).is_none();
                Touched::Pool {
                    venue,
                    tokens,
                    first,
                }
            }
            MarketEvent::Invalidated { venue, ticker } => {
                self.books.remove(&(venue.clone(), ticker.clone()));
                Touched::Invalidated { venue, ticker }
            }
            MarketEvent::CoinCheck(c) => {
                if let Some(addr) = c.asset.bsc_contract {
                    self.coin_checks.insert((c.venue.clone(), addr), c.clone());
                }
                Touched::CoinCheck(c)
            }
            MarketEvent::PoolCheck(c) => {
                self.pool_checks
                    .insert((c.venue.clone(), c.pool.clone()), c.clone());
                Touched::PoolCheck(c)
            }
            MarketEvent::SendChannel(s) => {
                self.channels.insert(s.id.clone(), s.clone());
                Touched::Channel(s)
            }
            MarketEvent::Notice { venue, code, text } => Touched::Notice { venue, code, text },
            MarketEvent::CheckPass {
                venue,
                ok,
                failed,
                waiting,
                not_configured: nc,
            } => Touched::CheckPass {
                venue,
                counts: [ok, failed, waiting, nc],
            },
            MarketEvent::WalletQueue(q) => {
                let prev = self.queues.insert(q.venue.clone(), q.state.clone());
                Touched::WalletQueue { queue: q, prev }
            }
        }
    }

    /// Удаляет пулы токена на площадке (DEX объявил данные монеты недостоверными).
    /// Принимает: площадку, адрес токена. Возвращает: сколько пулов удалено.
    pub fn drop_pools_of(&mut self, venue: &VenueId, token: Address) -> usize {
        let before = self.pools.len();
        self.pools
            .retain(|(v, _), p| !(v == venue && (p.token0 == token || p.token1 == token)));
        before - self.pools.len()
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
