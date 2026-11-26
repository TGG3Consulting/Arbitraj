//! Публикует стороны Hanji по новым головам Base; при выключении снимает только собственные уровни.
use super::{
    book::{self, Books, Config},
    live,
    routing::Tickets,
};
use crate::gateway::events::{MarketEvent, SourceId};
use crate::venues::bsc_dex::{hashflow::availability, parts::Parts};
use alloy_primitives::Address;
use chrono::Utc;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
#[derive(Default, Clone)]
pub(in crate::venues::bsc_dex) struct Cache {
    books: Arc<Mutex<Option<Arc<Books>>>>,
    pub(super) funds: availability::Cache,
    pub(super) tickets: Tickets,
}
impl Cache {
    pub(super) fn current(&self, p: &Parts) -> Option<Arc<Books>> {
        let b = self
            .books
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .clone()?;
        (b.revision == p.ctx.settings.generation()
            && b.expires > Utc::now().timestamp_millis()
            && p.heads.fresh(&p.ctx).ok()?.number == b.head.number.to::<u64>())
        .then_some(b)
    }
    fn swap(&self, b: Option<Arc<Books>>) -> Option<Arc<Books>> {
        std::mem::replace(
            &mut *self.books.lock().unwrap_or_else(|x| x.into_inner()),
            b,
        )
    }
}
pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    if p.hanji.swap(None).is_some() {
        let _ = out
            .send(MarketEvent::RfqMarkets {
                source: SourceId::new("Hanji"),
                venue: p.ctx.venue(),
                quote: Address::ZERO,
                markets: Default::default(),
                expires_ms: 0,
            })
            .await;
    }
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    let mut heads = p.heads.watch();
    let mut changes = p.ctx.settings.subscribe();
    let mut registry = None;
    loop {
        if let Some(cfg) = Config::read(&p) {
            let revision = p.ctx.settings.generation();
            let read = async {
                // Обновление списка рынков идет отдельно от получения стаканов по блоку.
                let at = Utc::now().timestamp();
                let ttl = i64::try_from(p.ctx.int("hanji.registry_refresh_s").ok()?).ok()?;
                if ttl <= 0 {
                    return None;
                }
                if registry
                    .as_ref()
                    .is_none_or(|(r, t, _): &(u64, i64, Vec<live::Market>)| {
                        *r != revision || at >= *t
                    })
                {
                    registry = Some((
                        revision,
                        at.saturating_add(ttl),
                        live::registry(&p.ctx).await.ok()?,
                    ));
                }
                book::load(&p, cfg.clone(), &registry.as_ref()?.2)
                    .await
                    .ok()
            };
            let b = tokio::time::timeout(cfg.age, read)
                .await
                .ok()
                .flatten()
                .filter(|b| {
                    b.revision == p.ctx.settings.generation()
                        && b.expires > Utc::now().timestamp_millis()
                });
            if let Some(b) = b {
                let mut markets = BTreeMap::new();
                for (&coin, books) in &b.by_coin {
                    let buy = books.iter().any(|v| !v.direction(cfg.cash).1.is_empty());
                    let sell = books.iter().any(|v| !v.direction(coin).1.is_empty());
                    if buy || sell {
                        markets.insert(coin, (buy, sell));
                    }
                }
                let expiry = b.expires;
                p.hanji.swap(Some(Arc::new(b)));
                if out
                    .send(MarketEvent::RfqMarkets {
                        source: SourceId::new("Hanji"),
                        venue: p.ctx.venue(),
                        quote: cfg.cash,
                        markets,
                        expires_ms: expiry,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            } else {
                clear(&p, &out).await;
            }
        } else {
            clear(&p, &out).await;
        }
        tokio::select! {r=heads.changed()=>{if r.is_err(){return;}},r=changes.recv()=>{if matches!(r,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}}}
    }
}
