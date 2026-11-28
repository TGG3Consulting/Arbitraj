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
#[path = "book_refresh.rs"]
mod refresh;

pub(in crate::venues::bsc_dex) async fn run(p: Parts, out: mpsc::Sender<MarketEvent>) {
    let (tx, rx) = mpsc::channel(1);
    tokio::select! {
        _ = refresh::read(&p, tx) => {},
        _ = refresh::publish(&p, rx, out) => {},
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
