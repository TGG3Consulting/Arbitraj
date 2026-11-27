//! Epsilon — самостоятельный способ оплатить один или два уже зарегистрированных V3 рынка.
//! https://github.com/alienbase-xyz/docs/blob/main/trading/epsilon.md
use super::{ctx::Ctx, hashflow, parts::Parts, quote::white_state};
use alloy_primitives::{Address, B256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
mod model;
mod profile;
pub(super) mod routing;
#[cfg(test)]
mod tests;
pub(super) mod trade;
pub(super) mod wire;
use profile::{Config, State};
pub(super) const LABEL: &str = "Epsilon";
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("epsilon.trading").unwrap_or(false)
}
type Tickets = HashMap<(Address, B256), routing::Ticket>;
type Snapshot = (u64, Instant, Option<Arc<State>>);
#[derive(Default, Clone)]
pub(super) struct Cache {
    state: Arc<Mutex<Option<Snapshot>>>,
    tickets: Arc<Mutex<Tickets>>,
    funds: hashflow::availability::Cache,
}
impl Cache {
    fn current(&self, p: &Parts, age: Duration) -> Option<Arc<State>> {
        let generation = p.ctx.settings.generation();
        let mut slot = self.state.lock().ok()?;
        if let Some((g, start, result)) = &*slot
            && *g == generation
            && start.elapsed() < age
        {
            return result.clone();
        }
        let config = Config::read(&p.ctx)?;
        let started = Instant::now();
        *slot = Some((generation, started, None));
        drop(slot);
        let storage = self.state.clone();
        let ctx = p.ctx.clone();
        tokio::spawn(async move {
            let work = async {
                let head = white_state::head(&ctx, "latest").await.ok()?;
                config.live(&ctx, head).await.map(Arc::new)
            };
            let result = tokio::time::timeout(age, work).await.ok().flatten();
            if let Ok(mut s) = storage.lock()
                && s.as_ref()
                    .is_some_and(|(g, t, _)| *g == generation && *t == started)
                && ctx.settings.generation() == generation
            {
                *s = Some((generation, started, result));
            }
        });
        None
    }
    fn save(&self, t: routing::Ticket) {
        let mut entries = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        entries.retain(|_, v| v.proof.plan.deadline > now);
        entries.insert((t.proof.call.payer, t.proof.call.calldata), t);
    }
    pub(super) fn take(&self, owner: Address, id: B256) -> Option<routing::Ticket> {
        self.tickets.lock().ok()?.remove(&(owner, id))
    }
}
