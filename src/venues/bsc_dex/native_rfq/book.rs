//! Уровни Native orderbook задают порции входного ERC20. Минимум API указан в сырых единицах.
//! https://docs.native.org/native-dev/build-with-native/swap-aggregators/firmquote-swap-apis/get-orderbook
use super::{Profile, levels::Depth};
use crate::{
    gateway::{
        events::{MarketEvent, SourceId},
        market_http,
    },
    venues::bsc_dex::{hashflow::availability, parts::Parts},
};
use alloy_primitives::{Address, B256};
use chrono::Utc;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Default)]
struct Inner {
    depth: Option<(u64, Instant, Arc<Depth>)>,
    tickets: HashMap<(Address, B256), Arc<super::routing::Ticket>>,
}
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Cache {
    inner: Arc<Mutex<Inner>>,
    pub(super) funds: availability::Cache,
}
impl Cache {
    pub(super) fn depth(&self, p: &Parts) -> Option<Arc<Depth>> {
        let age = p.ctx.int("native_rfq.max_age_ms").ok()?;
        let g = self.inner.lock().ok()?;
        let (rev, when, book) = g.depth.as_ref()?;
        (*rev == p.ctx.settings.generation() && when.elapsed() < Duration::from_millis(age))
            .then(|| book.clone())
    }
    pub(super) fn save(&self, t: super::routing::Ticket) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .tickets
            .retain(|_, q| q.expires() > Utc::now().timestamp().max(0) as u64);
        guard.tickets.insert(t.id(), Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(
        &self,
        w: Address,
        id: B256,
    ) -> Option<Arc<super::routing::Ticket>> {
        self.inner.lock().ok()?.tickets.remove(&(w, id))
    }
}

pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    let had = p
        .native_quotes
        .inner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .depth
        .take()
        .is_some();
    if had {
        let _ = out
            .send(MarketEvent::RfqMarkets {
                source: SourceId::new("Native RFQ"),
                venue: p.ctx.venue(),
                quote: Address::ZERO,
                markets: Default::default(),
                expires_ms: 0,
            })
            .await;
    }
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    let mut settings = p.ctx.settings.subscribe();
    loop {
        let initial = p.ctx.settings.generation();
        let cfg = || {
            let profile = Profile::read(&p.ctx).ok()?;
            profile.module?;
            Some((
                profile,
                p.ctx.int("native_rfq.max_age_ms").ok()?.max(1),
                p.tokens.usdt(&p.ctx).ok()?,
            ))
        };
        let Some((profile, age, quote)) = super::enabled(&p.ctx).then(cfg).flatten() else {
            clear(&p, &out).await;
            if matches!(
                settings.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
            continue;
        };
        let start = Instant::now();
        let stamp = Utc::now().timestamp_millis();
        let get = async {
            let mut url = super::client::url(&p.ctx, "orderbook").ok()?;
            url.query_pairs_mut().append_pair("chain", &profile.slug);
            let r = p.ctx.http.get(url);
            let (status, data) = market_http::exchange(super::HTTP_SERVICE, r, p.ctx.limits().ok()?)
                .await.ok()?;
            if !status.is_success() {
                return None;
            }
            Some(Arc::new(Depth::parse(
                &serde_json::from_str::<Value>(&data).ok()?,
            )?))
        };
        let book = tokio::time::timeout(Duration::from_millis(age), get)
            .await
            .ok()
            .flatten();
        if initial != p.ctx.settings.generation() {
            continue;
        }
        if let Some(b) = book {
            let markets = b.markets(quote, &p);
            p.native_quotes
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .depth = Some((initial, start, b));
            if out
                .send(MarketEvent::RfqMarkets {
                    source: SourceId::new("Native RFQ"),
                    venue: p.ctx.venue(),
                    quote,
                    markets,
                    expires_ms: stamp.saturating_add(i64::try_from(age).unwrap_or(i64::MAX)),
                })
                .await
                .is_err()
            {
                return;
            }
        } else {
            clear(&p, &out).await
        }
        tokio::select! {_ =tokio::time::sleep(Duration::from_millis((age/2).max(1)))=>{},c=settings.recv()=>if matches!(c,Err(tokio::sync::broadcast::error::RecvError::Closed)){return}}
    }
}
