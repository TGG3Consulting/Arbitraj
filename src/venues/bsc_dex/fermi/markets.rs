//! Хранит уровни Fermi одного строителя в памяти, отзывает их при смене настроек и публикует доступные стороны сделки ядру.
use super::super::{hashflow::availability, parts::Parts, pools::load_woo_ladder};
use super::{Builder, Frame, Head, levels::levels};
use crate::gateway::events::{MarketEvent, SourceId};
use alloy::primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Config {
    pub executor: Address,
    pub builder: Builder,
    pub interval: u64,
    pub timeout: u64,
    pub gas: Decimal,
    pub cash: Address,
    pub ladder: Vec<Decimal>,
}
impl Config {
    pub fn read(p: &Parts) -> Option<Self> {
        let ctx = &p.ctx;
        if ctx.chain_id() != Some(1) || !ctx.flag("fermi.trading").unwrap_or(false) {
            return None;
        }
        let executor = ctx.addr("fermi.executor").ok().filter(|a| !a.is_zero())?;
        let builder = Builder::parse(&ctx.text("fermi.builder").ok()?)?;
        let interval = ctx.int("fermi.books_interval_ms").ok().filter(|v| *v > 0)?;
        let timeout = ctx.int("fermi.quote_budget_ms").ok().filter(|v| *v > 0)?;
        let gas = ctx
            .dec("fermi.gas_budget_native")
            .ok()
            .filter(|v| *v > Decimal::ZERO)?;
        let cash = p.tokens.usdt(ctx).ok()?;
        let list = ctx.settings.list("fermi.quote_sizes", ctx.at()).ok()?;
        let ladder = load_woo_ladder::parse_sizes(ctx, "fermi.quote_sizes", &list)
            .ok()
            .filter(|v| !v.is_empty())?;
        Some(Self {
            executor,
            builder,
            interval,
            timeout,
            gas,
            cash,
            ladder,
        })
    }
}
#[derive(Default)]
pub(super) struct SideBook(pub(super) Vec<(Decimal, Decimal)>);
impl SideBook {
    pub(super) fn price(&self, qty: Decimal, buy: bool) -> Option<Decimal> {
        if qty <= Decimal::ZERO {
            return None;
        }
        let at = self.0.partition_point(|(q, _)| *q < qty);
        let (size, cash) = self.0.get(at)?;
        let marginal = cash.checked_div(*size)?;
        let conservative = if *size == qty || at == 0 {
            marginal
        } else {
            let (before, prev) = self.0[at - 1];
            let prev = prev.checked_div(before)?;
            if buy {
                prev.max(marginal)
            } else {
                prev.min(marginal)
            }
        };
        conservative.checked_mul(qty)
    }
    pub fn breaks(&self, max: Decimal) -> Vec<Decimal> {
        self.0
            .iter()
            .filter_map(|(q, _)| (*q <= max).then_some(*q))
            .collect()
    }
}
pub(super) struct Books {
    pub config: Config,
    pub head: Head,
    pub frame: Frame,
    pub revision: std::sync::atomic::AtomicU64,
    pub sides: BTreeMap<(Address, bool), SideBook>,
}
impl Books {
    pub fn current(&self) -> bool {
        self.frame.current(
            self.head.number.to(),
            self.head.timestamp.to(),
            Utc::now().timestamp().max(0) as u64,
        )
    }
}
#[derive(Default, Clone)]
pub(in crate::venues::bsc_dex) struct Cache {
    books: Arc<Mutex<Option<Arc<Books>>>>,
    pub(super) funds: availability::Cache,
    pub(super) tickets: super::routing::Tickets,
}
impl Cache {
    pub(super) fn current(&self, p: &Parts) -> Option<Arc<Books>> {
        let books = self
            .books
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .clone()?;
        use std::sync::atomic::Ordering;
        if !books.current() {
            return None;
        }
        let revision = p.ctx.settings.generation();
        if books.revision.load(Ordering::Relaxed) != revision {
            if Config::read(p).as_ref() != Some(&books.config)
                || revision != p.ctx.settings.generation()
            {
                return None;
            }
            books.revision.store(revision, Ordering::Relaxed);
        }
        Some(books)
    }
    fn swap(&self, new: Option<Arc<Books>>) -> Option<Arc<Books>> {
        std::mem::replace(
            &mut *self.books.lock().unwrap_or_else(|x| x.into_inner()),
            new,
        )
    }
}
pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    if p.slot_markets.swap(None).is_none() {
        return;
    }
    let _ = out
        .send(MarketEvent::RfqMarkets {
            source: SourceId::new("Fermi"),
            venue: p.ctx.venue(),
            quote: Address::ZERO,
            markets: Default::default(),
            expires_ms: 0,
        })
        .await;
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    if p.ctx.chain_id() != Some(1) {
        return;
    }
    let mut changes = p.ctx.settings.subscribe();
    let emit = |cash, markets, expires_ms| {
        out.send(MarketEvent::RfqMarkets {
            source: SourceId::new("Fermi"),
            venue: p.ctx.venue(),
            quote: cash,
            markets,
            expires_ms,
        })
    };
    let mut last = None;
    'poll: loop {
        let cfg = Config::read(&p);
        if (cfg != last || cfg.is_none())
            && p.slot_markets.swap_if_changed(cfg.as_ref())
            && emit(Address::ZERO, Default::default(), 0).await.is_err()
        {
            return;
        }
        last.clone_from(&cfg);
        let Some(config) = cfg.clone() else {
            if matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
            continue;
        };
        let read = tokio::time::timeout(
            Duration::from_millis(config.timeout),
            levels(&p, config.clone()),
        );
        tokio::pin!(read);
        let data = loop {
            tokio::select! {biased;
                change=changes.recv()=>{if matches!(change,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}
                    if Config::read(&p)!=cfg {continue 'poll;}}
                result=&mut read=>break result.ok().and_then(Result::ok).map(Arc::new)
            }
        };
        if Config::read(&p) != cfg {
            continue;
        }
        let mut markets = BTreeMap::new();
        let mut expiry = 0;
        if let Some(book) = &data {
            for &(token, buy) in book.sides.keys() {
                let side = markets.entry(token).or_insert((false, false));
                if buy {
                    side.0 = true;
                } else {
                    side.1 = true;
                }
            }
            expiry = i64::try_from(book.frame.timestamp.saturating_mul(1000)).unwrap_or(i64::MAX);
        }
        let present = !markets.is_empty();
        let prior = p.slot_markets.swap(data);
        if (present || prior.is_some()) && emit(config.cash, markets, expiry).await.is_err() {
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(config.interval);
        loop {
            tokio::select! {_=tokio::time::sleep_until(deadline)=>break,
                change=changes.recv()=>{if matches!(change,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}
                    if Config::read(&p)!=cfg {break;}}
            }
        }
    }
}
impl Cache {
    fn swap_if_changed(&self, cfg: Option<&Config>) -> bool {
        let mut books = self.books.lock().unwrap_or_else(|x| x.into_inner());
        if books.as_ref().is_some_and(|b| Some(&b.config) != cfg) {
            books.take();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interpolation_uses_worse_observed_unit_price_and_cannot_cross_depth() {
        let side = SideBook(vec![
            (Decimal::from(1), Decimal::from(4)),
            (Decimal::from(3), Decimal::from(18)),
        ]);
        assert_eq!(side.price(Decimal::from(2), true), Some(Decimal::from(12)));
        assert_eq!(side.price(Decimal::from(2), false), Some(Decimal::from(8)));
        assert_eq!(side.price(Decimal::from(4), true), None);
        assert_eq!(side.price(Decimal::ZERO, false), None);
        assert_eq!(side.breaks(Decimal::from(2)), vec![Decimal::ONE]);
    }
}
