//! Аттестация контракта владельца для WETH отдельно от официального нативного роутера.
use super::{IFlux, invalid};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx,
        pools::{direct_module, families::Proto, nami_batch as batch, registry::Entry},
    },
};
use alloy_primitives::keccak256;
use serde_json::Value;
pub(in crate::venues::bsc_dex) async fn module(
    ctx: &Ctx,
    e: &Entry,
    at: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let crate::core::types::PoolRef::Singleton { manager, id } = e.pool.reference else {
        return Err(invalid(ctx));
    };
    let f = e.family;
    let target = ctx.dex_addr("dex.router", f.id)?;
    let role = if f.proto == Proto::InfinityCl {
        batch::view(at, target, IFlux::fluxClCall {})
    } else {
        batch::view(at, target, IFlux::fluxBinCall {})
    };
    let views = [
        role,
        batch::view(at, target, IFlux::fluxVaultCall {}),
        batch::view(at, target, IFlux::wrappedCall {}),
        batch::view(
            at,
            target,
            IFlux::fluxMarketCall {
                id,
                token: e.pool.token1,
            },
        ),
    ];
    let (found, (), [m, v, w, raw]) = tokio::try_join!(
        super::market(ctx, f, manager, id, (e.pool.token0, e.pool.token1), at),
        direct_module::verify_code(ctx, f.id, target, at),
        batch::read(ctx, views)
    )?;
    let actual = if f.proto == Proto::InfinityCl {
        batch::decode(&m, IFlux::fluxClCall {})?
    } else {
        batch::decode(&m, IFlux::fluxBinCall {})?
    };
    if actual != manager
        || batch::decode(&v, IFlux::fluxVaultCall {})? != ctx.dex_addr("dex.vault", f.id)?
        || batch::decode(&w, IFlux::wrappedCall {})? != e.pool.token0
        || found.key != e.key
        || raw.len() != 192
        || keccak256(raw) != id
        || ctx.settings.generation() != generation
    {
        return Err(invalid(ctx));
    }
    Ok(())
}
