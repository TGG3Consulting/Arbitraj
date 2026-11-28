//! Для одной сети/площадки проверяет роли один раз на канонический родитель, модуль и поколение настроек.
//! Обе формы и оба строителя ждут одно чтение. Ошибка RPC или замена канонического блока требуют новой проверки.
use super::{Ctx, bop_cash::Path, bop_versions};
use crate::gateway::errors::GatewayResult;
use alloy::primitives::{Address, B256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::OnceCell;
#[derive(Default)]
struct Head {
    stamp: Option<(B256, u64)>,
    bridge: Arc<OnceCell<()>>,
    modules: BTreeMap<Address, Arc<OnceCell<()>>>,
}
#[derive(Default, Clone)]
pub(super) struct Cache(Arc<Mutex<Head>>);
impl Cache {
    pub(super) async fn verify(
        &self,
        ctx: &Ctx,
        hash: B256,
        revision: u64,
        path: Path,
    ) -> GatewayResult<()> {
        let (cell, bridge) = {
            let mut h = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if h.stamp != Some((hash, revision)) {
                h.modules.clear();
                h.bridge = Default::default();
                h.stamp = Some((hash, revision));
            }
            (
                h.modules.entry(path.module.address).or_default().clone(),
                h.bridge.clone(),
            )
        };
        cell.get_or_try_init(|| {
            path.identity.verify(
                ctx,
                hash,
                path.module,
                path.module.hash == bop_versions::CASH,
            )
        })
        .await?;
        if path.bridge {
            bridge
                .get_or_try_init(|| path.identity.verify_bridge(ctx, hash))
                .await?;
        }
        Ok(())
    }
}
