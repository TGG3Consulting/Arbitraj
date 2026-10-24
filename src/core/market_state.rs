//! Файл: core/market_state.rs
//! Что делает: всё, что ядро знает о рынке прямо сейчас (шаг 2 ТЗ): стаканы бирж, пулы DEX,
//! проверки налогов монет и крючков пулов, состояние каналов отправки, монеты с торгуемой спот-парой у биржи
//! (для подписки DEX, core/subscribe.rs) и состояние их пар (статус, покупка и продажа через API — для справочника,
//! core/presence.rs), монеты с найденными, но ещё не прочитанными пулами DEX — из событий адаптеров.
//! Данные, которые площадка объявила недостоверными, удаляются: по ним сделок нет.
//! Чтение состояния (стаканы, пулы, пары, проверки) — market_read.rs.
//! Кто вызывает: core/engine.rs (применяет события), registry.rs, signal.rs, prices.rs, позже executor.rs.
//! От чего зависит: core/types, gateway/events.
//! Настройки: нет. Свежесть цены проверяет адаптер при каждой котировке (feed.stale_after_ms): только он знает,
//! живо ли соединение (venues/bingx/book.rs); без свежей котировки ядро не торгует.

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;
use chrono::{DateTime, Utc};

use crate::core::types::{
    CoinCheck, OrderBook, Pool, PoolCheck, PoolRef, SendChannelStats, VenueId, WalletQueueState,
};
use crate::gateway::events::{MarketEvent, SpotPairState};

pub use super::touched::Touched;

/// Рыночное состояние в памяти.
#[derive(Default)]
pub struct MarketState {
    pub(super) books: HashMap<(VenueId, String), OrderBook>,
    pub(super) pools: HashMap<(VenueId, PoolRef), Pool>,
    /// Проверка налогов: ключ — площадка DEX и адрес контракта монеты.
    pub(super) coin_checks: HashMap<(VenueId, Address), CoinCheck>,
    pub(super) pool_checks: HashMap<(VenueId, PoolRef), PoolCheck>,
    /// Каналы отправки по id (в SendChannelStats площадки нет; каналы есть только у адаптера DEX).
    pub(super) channels: HashMap<String, SendChannelStats>,
    /// Последнее состояние очереди кошелька по площадке DEX — уведомления только на смену состояния.
    pub(super) queues: HashMap<VenueId, WalletQueueState>,
    /// Биржа → монеты с торгуемой спот-парой (MarketEvent::SpotPairs).
    pub(super) spot: HashMap<VenueId, HashSet<String>>,
    /// DEX → монеты, чьи пулы найдены, но ещё не прочитаны (MarketEvent::PoolsLoading, решение 61).
    pub(super) loading: HashMap<VenueId, HashSet<Address>>,
    /// Биржа → тикер → состояние пары (статус, покупка и продажа через API) из того же SpotPairs.
    pub(super) pairs: HashMap<VenueId, HashMap<String, SpotPairState>>,
    /// Биржа → когда её справочник пар последний раз получен (SpotPairs.loaded_at; свежесть — pairs_fresh.rs).
    pub(super) pairs_at: HashMap<VenueId, DateTime<Utc>>,
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
            MarketEvent::SpotPairs {
                venue,
                tickers,
                pairs,
                loaded_at,
            } => {
                self.pairs_at.insert(venue.clone(), loaded_at);
                let by_ticker = pairs.into_iter().map(|p| (p.ticker.clone(), p)).collect();
                self.pairs.insert(venue.clone(), by_ticker);
                self.spot.insert(venue, tickers.into_iter().collect());
                Touched::SpotPairs
            }
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
            MarketEvent::PoolsLoading { venue, coins } => {
                let now: HashSet<Address> = coins.into_iter().collect();
                let changed = self.loading.get(&venue) != Some(&now);
                self.loading.insert(venue, now);
                Touched::PoolsLoading { changed }
            }
            MarketEvent::WalletQueue(q) => {
                let prev = self.queues.insert(q.venue.clone(), q.state.clone());
                Touched::WalletQueue { queue: q, prev }
            }
        }
    }
}
