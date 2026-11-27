//! Публичная выдача Gladius. Ордера проверяются через настоящий Reactor и Quoter на одном блоке.
use super::{
    model::{ApiOrder, Offer},
    profile::Profile,
    routing::Ticket,
    *,
};
use crate::{
    gateway::{
        events::{MarketEvent, SourceId},
        market_http,
    },
    venues::bsc_dex::hashflow::availability,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
type Orders = Arc<Vec<Arc<Offer>>>;
#[derive(Default)]
struct State {
    book: Option<(u64, Instant, Orders)>,
    tickets: HashMap<(Address, B256), Arc<Ticket>>,
}
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Cache {
    inner: Arc<Mutex<State>>,
    pub(super) funds: availability::Cache,
}
impl Cache {
    pub(super) fn rows(&self, p: &Parts) -> Option<Orders> {
        let age = Duration::from_millis(p.ctx.int("gladius.max_age_ms").ok()?);
        let state = self.inner.lock().ok()?;
        let (g, at, rows) = state.book.as_ref()?;
        (*g == p.ctx.settings.generation() && at.elapsed() < age).then(|| rows.clone())
    }
    pub(super) fn save(&self, t: Ticket) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now().timestamp().max(0) as u64;
        state.tickets.retain(|_, q| q.offer.expiry > now);
        state.tickets.insert((t.owner, t.id()), Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(&self, payer: Address, id: B256) -> Option<Arc<Ticket>> {
        let mut state = self.inner.lock().ok()?;
        let t = state.tickets.remove(&(payer, id))?;
        state.tickets.retain(|_, q| q.offer.id() != t.offer.id());
        Some(t)
    }
}
fn known(p: &Parts, quote: Address) -> HashSet<Address> {
    p.tokens
        .all()
        .into_iter()
        .map(|t| t.address)
        .filter(|t| *t != quote)
        .collect()
}
fn markets(rows: &[Arc<Offer>], quote: Address) -> BTreeMap<Address, (bool, bool)> {
    let mut markets = BTreeMap::new();
    for r in rows {
        let (a, b) = r.pair();
        if a == quote {
            markets.entry(b).or_insert((false, false)).0 = true;
        } else if b == quote {
            markets.entry(a).or_insert((false, false)).1 = true;
        }
    }
    markets
}
async fn read(p: &Parts, profile: &Profile, quote: Address) -> Option<Orders> {
    let ctx = &p.ctx;
    let known = known(p, quote);
    let mut url = reqwest::Url::parse(&ctx.text("gladius.orders_url").ok()?).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    url.query_pairs_mut()
        .append_pair("chainId", &profile.chain.to_string())
        .append_pair("orderStatus", "open");
    let (status, body) = market_http::exchange("gladius", ctx.http.get(url), ctx.limits().ok()?)
        .await
        .ok()?;
    if !status.is_success() {
        return None;
    }
    let page: Value = serde_json::from_str(&body).ok()?;
    let orders = page.get("orders")?.as_array()?;
    if orders.is_empty() {
        return Some(Arc::new(Vec::new()));
    }
    let h = white_state::head(ctx, "latest").await.ok()?;
    profile.verify(ctx, h).await.ok()?;
    let domain = super::witness::Domain::read(ctx, profile, h).await?;
    let mut result = Vec::new();
    let mut hashes = HashSet::new();
    for raw in orders {
        let Ok(api) = serde_json::from_value::<ApiOrder>(raw.clone()) else {
            continue;
        };
        let Some(offer) = Offer::parse(api, profile, h.timestamp.to()) else {
            continue;
        };
        let (a, b) = offer.pair();
        if !(a == quote && known.contains(&b) || b == quote && known.contains(&a))
            || !timely(ctx, offer.expiry)
            || !hashes.insert(offer.api.order_hash)
        {
            continue;
        }
        result.push(offer);
    }
    use futures_util::{StreamExt, stream};
    let entries = stream::iter(result)
        .map(|mut offer| async move {
            if !domain.authentic(ctx, &offer, h).await {
                return None;
            }
            offer.quantity = offer
                .quantity
                .min(offer.funded(ctx, domain.permit, h).await?);
            if offer.quantity < offer.order.threshold.max(U256::ONE) {
                return None;
            }
            offer.debit = offer.quote(ctx, profile, offer.quantity, h).await.ok()?;
            Some(Arc::new(offer))
        })
        .buffer_unordered(4)
        .filter_map(std::future::ready)
        .collect::<Vec<_>>()
        .await;
    Some(Arc::new(entries))
}
pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    let had = p
        .gladius
        .inner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .book
        .take()
        .is_some();
    if had {
        let _ = out
            .send(MarketEvent::RfqMarkets {
                source: SourceId::new(LABEL),
                venue: p.ctx.venue(),
                quote: Address::ZERO,
                markets: BTreeMap::new(),
                expires_ms: 0,
            })
            .await;
    }
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    let mut changes = p.ctx.settings.subscribe();
    loop {
        let generation = p.ctx.settings.generation();
        let config = || {
            Some((
                Profile::read(&p.ctx)?,
                p.ctx.int("gladius.max_age_ms").ok()?.max(1),
                p.tokens.usdt(&p.ctx).ok()?,
            ))
        };
        let Some((profile, ms, quote)) = enabled(&p.ctx).then(config).flatten() else {
            clear(&p, &out).await;
            if matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
            continue;
        };
        let start = Instant::now();
        let stamp = Utc::now().timestamp_millis();
        let rows = tokio::time::timeout(Duration::from_millis(ms), read(&p, &profile, quote))
            .await
            .ok()
            .flatten();
        if generation != p.ctx.settings.generation() {
            continue;
        }
        if let Some(rows) = rows {
            let markets = markets(&rows, quote);
            p.gladius
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .book = Some((generation, start, rows));
            if out
                .send(MarketEvent::RfqMarkets {
                    source: SourceId::new(LABEL),
                    venue: p.ctx.venue(),
                    quote,
                    markets,
                    expires_ms: stamp.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX)),
                })
                .await
                .is_err()
            {
                return;
            }
        } else {
            clear(&p, &out).await;
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_millis((ms/2).max(1)))=>{}, r=changes.recv()=>if matches!(r,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}}
    }
}
