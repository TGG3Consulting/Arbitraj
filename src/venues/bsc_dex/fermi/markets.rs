//! Хранит уровни Fermi одного строителя в памяти, отзывает их при смене настроек и публикует доступные стороны сделки ядру.
use super::super::{hashflow::availability, parts::Parts};
pub(super) use super::slot_config::Config;
use super::{Frame, Head, levels::levels};
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
    pub revision: u64,
    pub prop: Option<super::super::exec::prop_wallet::Terms>,
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
    books: Arc<Mutex<Vec<Arc<Books>>>>,
    pub(super) funds: availability::Cache,
    pub(super) tickets: super::routing::Tickets,
}
impl Cache {
    pub(super) fn current(&self, p: &Parts) -> Vec<Arc<Books>> {
        let generation = p.ctx.settings.generation();
        self.books
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .iter()
            .filter(|b| b.revision == generation && b.current())
            .cloned()
            .collect()
    }
    fn replace(&self, new: Vec<Arc<Books>>) -> bool {
        let mut data = self.books.lock().unwrap_or_else(|x| x.into_inner());
        let had = !data.is_empty();
        *data = new;
        had
    }
}
pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    if p.slot_markets.replace(Vec::new()) {
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
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    if p.ctx.chain_id() != Some(1) {
        return;
    }
    let mut changes = p.ctx.settings.subscribe();
    loop {
        let rev = p.ctx.settings.generation();
        let configs = Config::all(&p);
        let Some(first) = configs.first() else {
            clear(&p, &out).await;
            if matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
            continue;
        };
        let (cash, interval, timeout) = (first.cash, first.interval, first.timeout);
        let ctx = &p.ctx;
        let mut books: Vec<Arc<Books>> = Vec::new();
        // One public snapshot for each selected builder; a slow or disabled publisher never supplies another venue's price.
        let read = async {
            let (chain, h) = tokio::try_join!(
                super::rpc::call::<alloy_primitives::U64>(
                    ctx,
                    "eth_chainId",
                    serde_json::json!([])
                ),
                super::rpc::call::<Head>(
                    ctx,
                    "eth_getBlockByNumber",
                    serde_json::json!(["latest", false])
                )
            )?;
            Ok::<_, crate::gateway::errors::GatewayError>((chain, h))
        };
        if let Ok(Ok((chain, h))) = tokio::time::timeout(Duration::from_millis(timeout), read).await
            && chain == alloy_primitives::U64::from(1)
        {
            let groups = [super::Builder::Titan, super::Builder::BuilderNet]
                .into_iter()
                .filter_map(|builder| {
                    let targets: Vec<_> = configs
                        .iter()
                        .filter(|c| c.builder == builder && c.permitted(ctx))
                        .cloned()
                        .collect();
                    if targets.is_empty() {
                        return None;
                    }
                    let p = &p;
                    let h = &h;
                    Some(async move {
                        let task = async {
                            let snap = super::builder_snapshot(ctx, builder).await.ok()?;
                            let active = targets
                                .into_iter()
                                .filter(|c| c.permitted(ctx) && rev == ctx.settings.generation());
                            let got = futures_util::future::join_all(
                                active.map(|c| levels(p, c, h, &snap, rev)),
                            )
                            .await;
                            Some(
                                got.into_iter()
                                    .filter_map(Result::ok)
                                    .filter(|b| b.current() && !b.sides.is_empty())
                                    .map(Arc::new)
                                    .collect::<Vec<_>>(),
                            )
                        };
                        tokio::time::timeout(Duration::from_millis(timeout), task)
                            .await
                            .ok()
                            .flatten()
                            .unwrap_or_default()
                    })
                });
            books = futures_util::future::join_all(groups)
                .await
                .into_iter()
                .flatten()
                .collect();
        }
        if rev != ctx.settings.generation() {
            clear(&p, &out).await;
            continue;
        }
        let mut markets = BTreeMap::new();
        let mut expiry = None;
        for book in &books {
            for &(coin, buy) in book.sides.keys() {
                let side = markets.entry(coin).or_insert((false, false));
                if buy {
                    side.0 = true;
                } else {
                    side.1 = true;
                }
            }
            expiry =
                Some(expiry.map_or(book.frame.timestamp, |t: u64| t.min(book.frame.timestamp)));
        }
        let available = !markets.is_empty();
        let prior = p.slot_markets.replace(books);
        if (available || prior)
            && out
                .send(MarketEvent::RfqMarkets {
                    source: SourceId::new("Fermi"),
                    venue: ctx.venue(),
                    quote: cash,
                    markets,
                    expires_ms: i64::try_from(expiry.unwrap_or(0).saturating_mul(1000))
                        .unwrap_or(i64::MAX),
                })
                .await
                .is_err()
        {
            return;
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(interval))=>{},changed=changes.recv()=>{
            if matches!(changed,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}
        }}
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
