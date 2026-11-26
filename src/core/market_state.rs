//! Файл: core/market_state.rs
//! Что делает: всё, что ядро знает о рынке прямо сейчас (шаг 2 ТЗ): стаканы бирж, пулы DEX,
//! проверки налогов монет и крючков пулов, состояние каналов отправки, монеты с торгуемой спот-парой у биржи
//! (для подписки DEX, core/subscribe.rs) и состояние их пар (статус, покупка и продажа через API — для справочника,
//! core/presence.rs), монеты с найденными, но ещё не прочитанными пулами DEX — из событий адаптеров.
//! Данные, которые площадка объявила недостоверными, удаляются: по ним сделок нет.
//! Решение 317 (доработка): у стакана — отметки последнего изменения содержимого (book_still.rs), не время кадра.
//! Отметки считаются только при включённом признаке marks_on (book_marks_on.rs: signal.dex_event_skip_hold или
//! log.signal_hold); выключен — на кадр стакана одна проверка признака, отметок нет (нагрузка как до решения 317).
//! Применение стакана и пула — market_apply.rs; чтение состояния (стаканы, пулы, пары, проверки) — market_read.rs.
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

type RfqSnapshot = (
    Address,
    std::collections::BTreeMap<Address, (bool, bool)>,
    i64,
);

/// Рыночное состояние в памяти.
#[derive(Default)]
pub struct MarketState {
    pub(super) rfq: HashMap<VenueId, HashMap<crate::gateway::events::SourceId, RfqSnapshot>>,
    pub(super) books: HashMap<(VenueId, String), OrderBook>,
    /// Решение 317 (доработка): когда последний раз менялось СОДЕРЖИМОЕ стакана (по префиксам уровней, book_still.rs);
    /// кадр с тем же содержимым отметки не двигает. Ключ — как у books.
    pub(super) book_marks: HashMap<(VenueId, String), super::book_still::BookMarks>,
    /// Решение 317 (ревью 16.11): отметки нужны (book_marks_on.rs). Выключено — note не вызывается, book_marks пуст.
    pub(super) marks_on: bool,
    /// Страховка решения 317: порог свежести стакана по площадке (venue.book_stale_ms; book_marks_on.rs) — кадр позже
    /// прошлого больше чем на него начинает отсчёт заново. Только при включённом признаке.
    pub(super) book_gap_ms: HashMap<VenueId, u64>,
    pub(super) pools: HashMap<(VenueId, PoolRef), Pool>,
    /// Индекс пулов по паре токенов (решение 129): (площадка, меньший адрес, больший адрес) → пулы. Справочник монет
    /// искал пулы пары проходом ВСЕХ пулов на каждую монету и промежуточную (O(монеты × пулы)) — у MEXC это тысячи ×
    /// тысячи на каждую пересборку.
    pub(super) pool_pairs: HashMap<(VenueId, Address, Address), HashSet<PoolRef>>,
    /// Проверка налогов: ключ — площадка DEX и адрес контракта монеты.
    pub(super) coin_checks: HashMap<(VenueId, Address), CoinCheck>,
    pub(super) pool_checks: HashMap<(VenueId, PoolRef), PoolCheck>,
    /// Каналы отправки по id (в SendChannelStats площадки нет; каналы есть только у адаптера DEX).
    pub(super) channels: HashMap<String, SendChannelStats>,
    /// Последнее состояние очереди кошелька по площадке DEX — уведомления только на смену состояния.
    /// Очереди кошельков DEX по (площадка, счёт) — решение 132: у каждого кошелька своя.
    pub(super) queues:
        HashMap<(VenueId, Option<crate::core::types::DexAccountId>), WalletQueueState>,
    /// Биржа → монеты с торгуемой спот-парой (MarketEvent::SpotPairs).
    pub(super) spot: HashMap<VenueId, HashSet<String>>,
    /// DEX → монеты, чьи пулы найдены, но ещё не прочитаны (MarketEvent::PoolsLoading, решение 61).
    pub(super) loading: HashMap<VenueId, HashSet<Address>>,
    /// Биржа → тикер → состояние пары (статус, покупка и продажа через API) из того же SpotPairs.
    pub(super) pairs: HashMap<VenueId, HashMap<String, SpotPairState>>,
    /// Биржа → когда её справочник пар последний раз получен (SpotPairs.loaded_at; свежесть — pairs_fresh.rs).
    pub(super) pairs_at: HashMap<VenueId, DateTime<Utc>>,
    /// Решение 227: тихая пара подтверждена по REST (решение 213) — стакан свежий для худшей цены ордера до этого
    /// момента (unix мс), как в котировке адаптера (book_confirmed). Ключ — (площадка, тикер площадки).
    pub(super) quiet: HashMap<(VenueId, String), i64>,
}

/// Ключ индекса пары: площадка и адреса по возрастанию. Ошибок нет.
pub(super) fn pair_key(venue: &VenueId, a: Address, b: Address) -> (VenueId, Address, Address) {
    if a <= b {
        (venue.clone(), a, b)
    } else {
        (venue.clone(), b, a)
    }
}

impl MarketState {
    pub(super) fn has_source(&self, venue: &VenueId, now: i64) -> bool {
        self.rfq.get(venue).is_some_and(|sources| {
            sources
                .values()
                .any(|(_, markets, expiry)| !markets.is_empty() && *expiry > now)
        })
    }
    pub(super) fn source_sides(
        &self,
        v: &VenueId,
        quote: Option<Address>,
        token: Address,
        now: i64,
    ) -> Option<(bool, bool)> {
        self.rfq
            .get(v)?
            .values()
            .filter_map(|(cash, markets, expires)| {
                (Some(*cash) == quote && *expires > now)
                    .then(|| markets.get(&token).copied())
                    .flatten()
            })
            .reduce(|a, b| (a.0 || b.0, a.1 || b.1))
    }
    pub(super) fn expire_source(
        &mut self,
        venue: &VenueId,
        source: crate::gateway::events::SourceId,
        expiry: i64,
    ) -> Option<std::collections::BTreeMap<Address, (bool, bool)>> {
        let group = self.rfq.get_mut(venue)?;
        if group.get(&source)?.2 != expiry {
            return None;
        }
        let (_, rows, _) = group.remove(&source)?;
        if group.is_empty() {
            self.rfq.remove(venue);
        }
        Some(rows)
    }

    /// Применяет событие адаптера. Принимает: событие. Возвращает: что изменилось. Ошибок нет.
    pub fn apply(&mut self, ev: MarketEvent) -> Touched {
        match ev {
            // Стакан и пул — market_apply.rs.
            MarketEvent::RfqMarkets {
                source,
                venue,
                quote,
                markets,
                expires_ms,
            } => {
                let group = self.rfq.entry(venue.clone()).or_default();
                let mut tokens = markets.keys().copied().collect::<Vec<_>>();
                let changed = group
                    .get(&source)
                    .map_or(!markets.is_empty(), |(q, prev, _)| {
                        q != &quote || prev != &markets
                    });
                if changed && let Some((_, old, _)) = group.get(&source) {
                    tokens.extend(old.keys().filter(|t| !markets.contains_key(*t)).copied());
                }
                let expiration = (!markets.is_empty()).then_some(expires_ms);
                if markets.is_empty() {
                    group.remove(&source);
                    if group.is_empty() {
                        self.rfq.remove(&venue);
                    }
                } else {
                    group.insert(source.clone(), (quote, markets, expires_ms));
                }
                Touched::RfqMarkets {
                    source,
                    venue,
                    tokens,
                    changed,
                    expires_ms: expiration,
                }
            }
            MarketEvent::Book(b) => self.apply_book(b),
            MarketEvent::Pool(p) => self.apply_pool(p),
            MarketEvent::Invalidated {
                venue,
                ticker,
                address,
            } => {
                self.books.remove(&(venue.clone(), ticker.clone()));
                self.book_marks.remove(&(venue.clone(), ticker.clone()));
                Touched::Invalidated {
                    venue,
                    ticker,
                    address,
                }
            }
            MarketEvent::CoinCheck(c) => {
                if let Some(addr) = c.asset.contract {
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
            MarketEvent::ExitGap(g) => Touched::ExitGap(g),
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
                loading,
            } => Touched::CheckPass {
                venue,
                counts: [ok, failed, waiting, nc, loading],
            },
            MarketEvent::PoolsLoading { venue, coins } => {
                let now: HashSet<Address> = coins.into_iter().collect();
                let changed = self.loading.get(&venue) != Some(&now);
                self.loading.insert(venue, now);
                Touched::PoolsLoading { changed }
            }
            MarketEvent::WalletQueue(q) => {
                let key = (q.venue.clone(), q.account.clone());
                let prev = self.queues.insert(key, q.state.clone());
                Touched::WalletQueue { queue: q, prev }
            }
            MarketEvent::Balance(b) => Touched::Balance(b),
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::gateway::events::SourceId;
    #[test]
    fn refreshing_or_expiring_one_source_preserves_the_other_side() {
        let mut state = MarketState::default();
        let venue = VenueId("eth_dex".into());
        let token = Address::repeat_byte(1);
        let quote = Address::repeat_byte(2);
        let event = |source, side, expires_ms| MarketEvent::RfqMarkets {
            source,
            venue: venue.clone(),
            quote,
            markets: std::collections::BTreeMap::from([(token, side)]),
            expires_ms,
        };
        state.apply(event(SourceId::new("Hashflow"), (true, false), 100));
        state.apply(event(SourceId::new("future-provider"), (false, true), 90));
        assert_eq!(
            state.source_sides(&venue, Some(quote), token, 80),
            Some((true, true))
        );
        assert_eq!(
            state.source_sides(&venue, Some(quote), token, 95),
            Some((true, false))
        );
        state.apply(event(SourceId::new("future-provider"), (false, true), 105));
        assert!(
            state
                .expire_source(&venue, SourceId::new("future-provider"), 90)
                .is_none()
        );
        assert!(
            state
                .expire_source(&venue, SourceId::new("Hashflow"), 100)
                .is_some()
        );
        assert_eq!(
            state.source_sides(&venue, Some(quote), token, 101),
            Some((false, true))
        );
        assert_eq!(state.source_sides(&venue, Some(token), token, 101), None);
        let legacy = serde_json::json!({"RfqMarkets":{"venue":"eth_dex","quote":quote,"markets":{},"expires_ms":0}});
        state.apply(event(SourceId::new("Hashflow"), (true, false), 109));
        state.apply(serde_json::from_value(legacy).unwrap());
        assert!(state.has_source(&venue, 101));
        assert!(!state.has_source(&VenueId("other-net".into()), 101));
        assert!(!state.has_source(&venue, 105));
        assert_eq!(
            state.source_sides(&venue, Some(quote), token, 101),
            Some((false, true))
        );
        state.apply(MarketEvent::RfqMarkets {
            source: SourceId::new("future-provider"),
            venue: venue.clone(),
            quote,
            markets: Default::default(),
            expires_ms: 0,
        });
        assert!(!state.rfq.contains_key(&venue));
    }
}
