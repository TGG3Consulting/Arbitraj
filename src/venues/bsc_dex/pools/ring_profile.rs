//! Параметры Ring берутся из настроек конкретной DEX. Для обмена требуется проверка контрактов на узле.
use super::super::ctx::Ctx;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, B256};
use rust_decimal::Decimal;
#[cfg(test)]
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Eq)]
pub struct Layout {
    pub factory: Address,
    pub few: Address,
    pub pair_init: B256,
    pub wrapper_init: B256,
    pub pair_runtime: B256,
    pub wrapper_runtime: B256,
    pub multicall: Address,
    pub direct: Vec<Address>,
    pub native: Address,
    pub quoter: Address,
    pub router: Address,
    pub permit: Address,
    pub infrastructure: [B256; 6],
}

impl Layout {
    #[cfg(test)]
    pub(super) fn archived(row: &Value) -> Option<Self> {
        let s = &row["settings"];
        if s["dex.kind"] != "ring_few_v2" {
            return None;
        }
        let init = s["dex.pool_code_hashes"].as_array()?;
        if init.len() != 1 {
            return None;
        }
        Some(Self {
            native: row["venue_settings"]["dex.native_wrapped"]
                .as_str()?
                .parse()
                .ok()?,
            quoter: s["dex.router"].as_str()?.parse().ok()?,
            router: s["dex.universal_router"].as_str()?.parse().ok()?,
            permit: s["dex.permit2"].as_str()?.parse().ok()?,
            infrastructure: s["dex.ring_infrastructure_hashes"]
                .as_array()?
                .iter()
                .map(|v| v.as_str()?.parse().ok())
                .collect::<Option<Vec<_>>>()?
                .try_into()
                .ok()?,
            factory: s["dex.factory"].as_str()?.parse().ok()?,
            few: s["dex.few_factory"].as_str()?.parse().ok()?,
            pair_init: init[0].as_str()?.parse().ok()?,
            wrapper_init: s["dex.few_init_hash"].as_str()?.parse().ok()?,
            pair_runtime: s["dex.pair_runtime_hash"].as_str()?.parse().ok()?,
            wrapper_runtime: s["dex.few_runtime_hash"].as_str()?.parse().ok()?,
            multicall: row["venue_settings"]["dex.multicall3"]
                .as_str()?
                .parse()
                .ok()?,
            direct: s["dex.ring_direct_assets"]
                .as_array()?
                .iter()
                .map(|x| x.as_str()?.parse().ok())
                .collect::<Option<Vec<_>>>()?,
        })
    }
}

pub fn active(ctx: &Ctx, id: &str) -> GatewayResult<Layout> {
    let invalid = |key: &str| {
        ctx.err(
            ErrorCode::NotConfigured,
            format!("Ring {id}: проверьте {key}"),
        )
    };
    if ctx.dex_text("dex.universal_router_version", id)? != "ring_few"
        || ctx.dex_text("dex.v2_fee_source", id)? != "fixed"
        || ctx.dex_dec("dex.v2_fee_pct", id)? != Decimal::new(3, 1)
    {
        return Err(invalid(
            "версию Universal Router и комиссию проверенного вида пары (0,3%)",
        ));
    }
    let hash = |v: &str| {
        v.parse::<B256>()
            .ok()
            .filter(|h| !h.is_zero() && *h != alloy_primitives::keccak256([]))
    };
    let read_hash = |k| hash(&ctx.dex_text(k, id)?).ok_or_else(|| invalid(k));
    let init = ctx.dex_list("dex.pool_code_hashes", id)?;
    let [pair] = init.as_slice() else {
        return Err(invalid("dex.pool_code_hashes"));
    };
    let infrastructure = ctx
        .dex_list("dex.ring_infrastructure_hashes", id)?
        .iter()
        .map(|v| hash(v))
        .collect::<Option<Vec<_>>>()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| invalid("dex.ring_infrastructure_hashes"))?;
    let direct: Vec<Address> = ctx
        .dex_list("dex.ring_direct_assets", id)?
        .iter()
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()
        .ok_or_else(|| invalid("dex.ring_direct_assets"))?;
    let layout = Layout {
        native: ctx.addr("dex.native_wrapped")?,
        factory: ctx.dex_addr("dex.factory", id)?,
        few: ctx.dex_addr("dex.few_factory", id)?,
        quoter: ctx.dex_addr("dex.router", id)?,
        router: ctx.dex_addr("dex.universal_router", id)?,
        permit: ctx.dex_addr("dex.permit2", id)?,
        multicall: ctx.addr("dex.multicall3")?,
        pair_init: hash(pair).ok_or_else(|| invalid("dex.pool_code_hashes"))?,
        wrapper_init: read_hash("dex.few_init_hash")?,
        pair_runtime: read_hash("dex.pair_runtime_hash")?,
        wrapper_runtime: read_hash("dex.few_runtime_hash")?,
        direct,
        infrastructure,
    };
    let unique = |v: &[Address]| {
        !v.contains(&Address::ZERO) && v.iter().collect::<BTreeSet<_>>().len() == v.len()
    };
    let h = [
        layout.pair_init,
        layout.wrapper_init,
        layout.pair_runtime,
        layout.wrapper_runtime,
    ];
    if layout.native.is_zero()
        || layout.contracts().contains(&layout.native)
        || !unique(&layout.contracts())
        || !unique(&layout.direct)
        || h.iter()
            .chain(&layout.infrastructure)
            .collect::<BTreeSet<_>>()
            .len()
            != 10
    {
        return Err(invalid("нулевой или повторяющийся адрес/хеш"));
    }
    Ok(layout)
}

impl Layout {
    pub(super) fn contracts(&self) -> [Address; 6] {
        [
            self.factory,
            self.few,
            self.quoter,
            self.router,
            self.permit,
            self.multicall,
        ]
    }
}
