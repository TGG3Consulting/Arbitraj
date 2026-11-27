//! Смена подтверждённого заголовка закрывает все лестницы площадки до обновления их конкретной пары.
use super::registry::Registry;
use crate::core::types::ambient::Stamp;
use alloy_primitives::B256;
use std::sync::{Arc, atomic::Ordering};
pub(crate) struct QuoteStamp {
    tip: Stamp,
    refs: Arc<std::collections::HashSet<crate::core::types::PoolRef>>,
}
impl Registry {
    pub(super) fn sync_ambient(&self, pools: &super::registry_index::Pools) {
        let enabled = !pools.ambient_refs.is_empty();
        self.has_ambient.store(enabled, Ordering::Release);
        if !enabled {
            pools.ambient_clock.advance(None);
        }
    }
    pub(in crate::venues::bsc_dex) fn observed_ambient(&self) -> Option<QuoteStamp> {
        if !self.watches_ambient() {
            return None;
        }
        let p = self.inner.read().unwrap_or_else(|e| e.into_inner());
        Some(QuoteStamp {
            tip: p.ambient_clock.observed()?,
            refs: p.ambient_refs.clone(),
        })
    }
    pub(in crate::venues::bsc_dex) fn route_was_current(
        &self,
        route: &crate::core::types::DexRoute,
        start: Option<&QuoteStamp>,
    ) -> bool {
        if start.is_none() && !self.watches_ambient() {
            return true;
        }
        let pools = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let hops = route.legs.iter().flat_map(|leg| &leg.hops);
        for hop in hops {
            let watched = pools.ambient_refs.contains(&hop.pool)
                || start.is_some_and(|s| s.refs.contains(&hop.pool));
            if watched {
                let Some(entry) = pools.get(&hop.pool) else {
                    return false;
                };
                let Some(stamp)=entry.pool.state.singleton_stamp() else {
                    return false;
                };
                if !entry.valid || !stamp.current() || !start.is_some_and(|s| s.tip.current())
                {
                    return false;
                }
            }
        }
        true
    }
    pub(in crate::venues::bsc_dex) fn watches_ambient(&self) -> bool {
        self.has_ambient.load(Ordering::Acquire)
    }
    pub(super) fn ambient_stamp(&self, number: u64, hash: B256) -> Stamp {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ambient_clock
            .stamp(number, hash)
    }
    pub(in crate::venues::bsc_dex) fn ambient_head(&self, tip: Option<(u64, B256)>) -> bool {
        let mut pools = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if !pools.ambient_clock.advance(tip) {
            return false;
        }
        let stale: Vec<_> = pools
            .ambient_refs
            .iter()
            .filter(|r| pools.get(r).is_some_and(|e| e.valid))
            .cloned()
            .collect();
        let mut pairs = Vec::with_capacity(stale.len());
        for r in &stale {
            if let Some(pair) = pools.update(r, |e| {
                e.valid = false;
                e.state_gen = self.next_gen();
                (e.coin, e.usdt)
            }) {
                pairs.push(pair);
            }
        }
        if !stale.is_empty() {
            self.graph.bg().changes.mark_all(&stale);
            self.version.fetch_add(1, Ordering::AcqRel);
        }
        drop(pools);
        if !pairs.is_empty() {
            self.shallow.wrote(&pairs);
        }
        true
    }
}

#[cfg(test)]
#[path = "ambient_head_tests.rs"]
mod tests;
