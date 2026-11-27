//! Страница ордеров и оплачиваемые предложения разделены по кошельку, сети и поколению настроек.
use super::*;
use crate::gateway::events::{MarketEvent, SourceId};
use crate::venues::bsc_dex::hashflow::availability;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
type Rows = Arc<Vec<Arc<Live>>>;
#[derive(Default)]
struct State {
    rows: Option<(u64, Instant, Rows)>,
    tickets: HashMap<(Address, B256), Arc<routing::Ticket>>,
}
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Cache {
    state: Arc<Mutex<State>>,
    pub(super) funds: availability::Cache,
}
impl Cache {
    pub(super) fn rows(&self, ctx: &Ctx) -> Option<Rows> {
        let max = Duration::from_millis(ctx.int("raindex.max_age_ms").ok()?);
        self.state
            .lock()
            .ok()?
            .rows
            .as_ref()
            .filter(|(g, at, _)| *g == ctx.settings.generation() && at.elapsed() < max)
            .map(|(_, _, r)| r.clone())
    }
    pub(super) fn save(&self, t: routing::Ticket) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        state.tickets.retain(|_, t| t.expiry > now);
        state.tickets.insert((t.owner, t.id()), Arc::new(t));
    }
    pub(in crate::venues::bsc_dex) fn take(
        &self,
        who: Address,
        id: B256,
    ) -> Option<Arc<routing::Ticket>> {
        self.state.lock().ok()?.tickets.remove(&(who, id))
    }
}
pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    use futures_util::{StreamExt, stream};
    let mut changes = p.ctx.settings.subscribe();
    loop {
        let ctx = &p.ctx;
        let g = ctx.settings.generation();
        let started = Instant::now();
        let config = || {
            Some((
                Profile::list(ctx)?,
                ctx.int("raindex.max_age_ms").ok()?,
                p.tokens.usdt(ctx).ok()?,
            ))
        };
        let active = enabled(ctx).then(config).flatten();
        let mut published = None;
        if let Some((sources, ms, cash)) = active.as_ref()
            && let Ok(head) = white_state::head(ctx, "latest").await
        {
            let ttl = Duration::from_millis(*ms);
            // Каждая книга может временно потерять индексатор независимо от соседних.
            let groups: Vec<_> = stream::iter(sources.clone())
                .map(|src| {
                    let p = &p;
                    let cash = *cash;
                    async move {
                        tokio::time::timeout(ttl / 2, src.read(p, head, cash))
                            .await
                            .ok()
                            .flatten()
                    }
                })
                .buffer_unordered(4)
                .collect()
                .await;
            if started.elapsed() < ttl && ctx.settings.generation() == g {
                let rows = groups
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(Arc::new)
                    .collect::<Vec<_>>();
                let mut markets = BTreeMap::new();
                for r in &rows {
                    let b = r.leg.input == *cash;
                    let coin = if b { r.leg.output } else { r.leg.input };
                    let sides = markets.entry(coin).or_insert((false, false));
                    if b {
                        sides.0 = true;
                    } else {
                        sides.1 = true;
                    }
                }
                p.raindex
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .rows = Some((g, started, Arc::new(rows)));
                let remaining = ttl.saturating_sub(started.elapsed());
                published = Some((
                    *cash,
                    markets,
                    chrono::Utc::now()
                        .timestamp_millis()
                        .saturating_add(i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX)),
                ));
            }
        }
        if published.is_none()
            && p.raindex
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .rows
                .take()
                .is_some()
        {
            published = Some((Address::ZERO, BTreeMap::new(), 0));
        }
        if let Some((quote, markets, expires_ms)) = published
            && out
                .send(MarketEvent::RfqMarkets {
                    source: SourceId::new(LABEL),
                    venue: ctx.venue(),
                    quote,
                    markets,
                    expires_ms,
                })
                .await
                .is_err()
        {
            return;
        }
        let wait = async {
            if let Some((_, ms, _)) = active {
                tokio::time::sleep(Duration::from_millis((ms / 2).max(1))).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {_=wait=>{},v=changes.recv()=>if matches!(v,Err(tokio::sync::broadcast::error::RecvError::Closed)){return;}}
    }
}

pub(in crate::venues::bsc_dex) async fn clear(p: &Parts, out: &mpsc::Sender<MarketEvent>) {
    let had = {
        let mut state = p.raindex.state.lock().unwrap_or_else(|e| e.into_inner());
        state.tickets.clear();
        state.rows.take().is_some()
    };
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
