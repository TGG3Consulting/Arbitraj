//! Выбирает доступные составные пути Fluid и Motoswap из индекса реестра для котировки.
use super::{
    fluid_dex_lite::shape,
    registry::{Entry, Registry},
};

impl Registry {
    pub(in crate::venues::bsc_dex) fn multi_market_edges(&self) -> Vec<std::sync::Arc<Entry>> {
        if !self
            .has_market_paths
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Vec::new();
        }
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut edges: Vec<_> = inner
            .path_refs
            .iter()
            .filter_map(|r| inner.get(r))
            .filter(|e| {
                e.valid
                    && !e.asleep
                    && (matches!(&e.pool.state,crate::core::types::PoolState::Motoswap(s) if (s.path_module.is_some() || s.fee_router_frozen) && crate::venues::bsc_dex::exec::motoswap_plan::shape(e).is_some()) || shape(e).is_some_and(|s| {
                        s.stamp.current()
                            && s.hop_module.is_some()
                            && s.levels.iter().any(|l| !l.is_empty())
                    }))
            })
            .cloned()
            .collect();
        edges.sort_by_key(|e| {
            (
                e.pool.reference.address(),
                shape(e).and_then(|s| s.market.id()),
            )
        });
        edges
    }
}
