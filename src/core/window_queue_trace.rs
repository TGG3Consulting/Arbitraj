//! Наблюдение одного отказа из-за занятой монеты по направлению в каждом открытом окне.
//! Только рынок, уже доставленный ядру. Индекс наблюдаемых монет меняется под замком windows.
use super::coin_key::CoinKey;
use super::engine::{Core, lk};
use crate::core::types::{Asset, Direction, VenueId};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueTrace {
    pub direction: Direction,
    pub busy_at: DateTime<Utc>,
    /// First observation that both the outer coin lock and all rides were free. A top-up may queue sooner.
    pub released_at: Option<DateTime<Utc>>,
    pub book_received_at: Option<DateTime<Utc>>,
    pub book_exchange_time: Option<DateTime<Utc>>,
    pub book_venue: Option<VenueId>,
    pub book_delivered_at: Option<DateTime<Utc>>,
    pub pool_delivered: Option<(DateTime<Utc>, u64)>,
    pub queued_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub result_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
    pub model_profit_usdt: Option<Decimal>,
}

impl QueueTrace {
    pub(super) fn new(d: &Direction) -> Self {
        Self {
            direction: d.clone(),
            busy_at: Utc::now(),
            released_at: None,
            book_received_at: None,
            book_exchange_time: None,
            book_venue: None,
            book_delivered_at: None,
            pool_delivered: None,
            queued_at: None,
            started_at: None,
            result_at: None,
            result: None,
            model_profit_usdt: None,
        }
    }
}

pub(super) fn blocked(core: &Core, d: &Direction) {
    let k = CoinKey::of(&d.asset);
    let mut windows = lk(&core.inner.windows);
    let Some(rows) = windows.queue_traces(&k) else {
        return;
    };
    if rows.iter().any(|r| &r.direction == d) {
        return;
    }
    rows.push(QueueTrace::new(d));
    let watched = core.inner.queue_trace_coins.load();
    if !watched.contains(&k) {
        let mut keys = (**watched).clone();
        keys.insert(k);
        core.inner
            .queue_trace_coins
            .store(std::sync::Arc::new(keys));
    }
}

/// Called while the window lock is held, so a new window for the same coin cannot be removed.
pub(super) fn forget(core: &Core, key: &CoinKey) {
    let watched = core.inner.queue_trace_coins.load();
    if watched.contains(key) {
        let mut keys = (**watched).clone();
        keys.remove(key);
        core.inner
            .queue_trace_coins
            .store(std::sync::Arc::new(keys));
    }
}

fn update(core: &Core, key: &CoinKey, mut change: impl FnMut(&mut QueueTrace)) {
    if !core.inner.queue_trace_coins.load().contains(key) {
        return;
    }
    if let Some(rows) = lk(&core.inner.windows).queue_traces(key) {
        for row in rows.iter_mut().filter(|r| r.result_at.is_none()) {
            change(row);
        }
    }
}

pub(super) fn released(core: &Core, key: &CoinKey) {
    update(core, key, |r| {
        r.released_at.get_or_insert_with(Utc::now);
    });
}

fn observes(r: &QueueTrace, venue: &VenueId, at: DateTime<Utc>) -> bool {
    (&r.direction.buy_on == venue || &r.direction.sell_on == venue)
        && r.released_at
            .or(r.queued_at)
            .is_some_and(|start| at >= start)
}

pub(super) fn book(
    core: &Core,
    asset: &Asset,
    venue: &VenueId,
    received: DateTime<Utc>,
    exchange: Option<DateTime<Utc>>,
) {
    if core.inner.queue_trace_coins.load().is_empty() {
        return;
    }
    let at = Utc::now();
    update(core, &CoinKey::of(asset), |r| {
        if r.book_delivered_at.is_none() && observes(r, venue, at) {
            r.book_delivered_at = Some(at);
            r.book_received_at = Some(received);
            r.book_exchange_time = exchange;
            r.book_venue = Some(venue.clone());
        }
    });
}

pub(super) fn pool(core: &Core, asset: &Asset, venue: &VenueId, mark: (DateTime<Utc>, u64)) {
    if core.inner.queue_trace_coins.load().is_empty() {
        return;
    }
    update(core, &CoinKey::of(asset), |r| {
        if observes(r, venue, mark.0) {
            r.pool_delivered.get_or_insert(mark);
        }
    });
}

pub(super) fn queued(core: &Core, d: &Direction) {
    if core.inner.queue_trace_coins.load().is_empty() {
        return;
    }
    update(core, &CoinKey::of(&d.asset), |r| {
        if &r.direction == d {
            r.queued_at.get_or_insert_with(Utc::now);
        }
    });
}

pub(super) fn started(core: &Core, d: &Direction) -> Option<DateTime<Utc>> {
    if core.inner.queue_trace_coins.load().is_empty() {
        return None;
    }
    let mut ticket = None;
    update(core, &CoinKey::of(&d.asset), |r| {
        if &r.direction == d && r.queued_at.is_some() && r.started_at.is_none() {
            r.started_at = Some(Utc::now());
            ticket = r.started_at;
        }
    });
    ticket
}

pub(super) fn result(
    core: &Core,
    d: &Direction,
    start: Option<DateTime<Utc>>,
    why: &str,
    pnl: Option<Decimal>,
) {
    if core.inner.queue_trace_coins.load().is_empty() {
        return;
    }
    update(core, &CoinKey::of(&d.asset), |r| {
        if &r.direction == d && r.queued_at.is_some() && r.started_at == start {
            r.result_at = Some(Utc::now());
            r.result = Some(why.to_string());
            r.model_profit_usdt = pnl;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        market_state::{MarketState, Touched},
        types::OrderBook,
    };
    use crate::gateway::events::MarketEvent;

    #[tokio::test]
    async fn only_post_release_market_deliveries_enter_the_contended_window() {
        let dir = std::env::temp_dir().join(format!(
            "arb-queue-trace-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let core = crate::core::test_core::open(&dir).await;
        let asset = Asset {
            chain: None,
            contract: None,
            ticker: "4STOCK".into(),
        };
        let d = Direction {
            asset: asset.clone(),
            buy_on: VenueId("kucoin".into()),
            sell_on: VenueId("bsc_dex".into()),
        };
        let key = CoinKey::of(&asset);
        lk(&core.inner.windows).pass(
            &d,
            "control".into(),
            true,
            Decimal::ONE,
            std::slice::from_ref(&d.buy_on),
            Utc::now(),
        );
        blocked(&core, &d);
        let before = Utc::now() - chrono::Duration::seconds(2);
        released(&core, &key);
        pool(&core, &asset, &d.sell_on, (before, 10));
        book(&core, &asset, &VenueId("other".into()), before, None);
        assert!(
            lk(&core.inner.windows).get(&key).unwrap().queue_traces[0]
                .pool_delivered
                .is_none()
        );
        let now = Utc::now();
        let market = OrderBook {
            venue: d.buy_on.clone(),
            asset: asset.clone(),
            bids: vec![],
            asks: vec![],
            exchange_time: Some(before),
            received_at: now,
        };
        let Touched::Book {
            venue,
            received_at,
            exchange_time,
            ..
        } = MarketState::default().apply(MarketEvent::Book(market))
        else {
            panic!("book was not delivered")
        };
        book(&core, &asset, &venue, received_at, exchange_time);
        pool(&core, &asset, &d.sell_on, (now, 11));
        queued(&core, &d);
        let ticket = started(&core, &d);
        assert!(ticket.is_some());
        result(&core, &d, ticket, "accept", Some(Decimal::ONE));
        let w = lk(&core.inner.windows).get(&key).unwrap();
        let r = &w.queue_traces[0];
        assert_eq!(
            (
                r.book_received_at,
                r.book_exchange_time,
                r.book_venue.as_ref()
            ),
            (Some(now), Some(before), Some(&d.buy_on))
        );
        assert_eq!(r.pool_delivered, Some((now, 11)));
        assert_eq!(r.model_profit_usdt, Some(Decimal::ONE));
    }
}
