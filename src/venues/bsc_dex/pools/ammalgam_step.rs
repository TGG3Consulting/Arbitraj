//! Выбор цели и проверка платёжного шага Ammalgam перед отправкой.
use super::{
    ammalgam::{IAmmalgam, bad, market},
    nami_batch as batch,
};
use crate::{core::types::PoolRef, gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::Address;
use serde_json::Value;
pub(crate) fn graph_target(ctx: &Ctx, e: &super::registry::Entry) -> Option<Address> {
    if !e.family.variant.ammalgam()
        || !matches!(e.pool.state, crate::core::types::PoolState::Ammalgam(_))
        || !matches!(&e.pool.reference,PoolRef::Contract(p) if !p.is_zero())
        || e.family.executor_kind != Some(super::exec_kind::ExecKind::Basin)
        || ctx.chain_id() != Some(crate::settings::evm_profiles::ETHEREUM_MAINNET)
    {
        return None;
    }
    let module = ctx.dex_addr("dex.router", e.family.id).ok()?;
    (!module.is_zero()
        && super::direct_module::configured(ctx, e.family.id).is_ok_and(|h| h.len() == 1))
    .then_some(module)
}
pub(crate) async fn check_step(
    ctx: &Ctx,
    e: &super::registry::Entry,
    input: Address,
    at: &Value,
) -> GatewayResult<()> {
    let module = graph_target(ctx, e)
        .filter(|_| [e.pool.token0, e.pool.token1].contains(&input))
        .ok_or_else(|| bad(ctx))?;
    let id = e.family.id;
    let generation = ctx.settings.generation();
    let q = [
        batch::view(at, module, IAmmalgam::factoryCall {}),
        batch::view(at, module, IAmmalgam::helperCall {}),
    ];
    let (_, (), [factory, helper]) = tokio::try_join!(
        market(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.pool.token0, e.pool.token1),
            at
        ),
        super::direct_module::verify_code(ctx, id, module, at),
        batch::read(ctx, q)
    )?;
    if batch::decode(&factory, IAmmalgam::factoryCall {})? != ctx.dex_addr("dex.factory", id)?
        || batch::decode(&helper, IAmmalgam::helperCall {})? != ctx.dex_addr("dex.quoter", id)?
        || generation != ctx.settings.generation()
    {
        return Err(bad(ctx));
    }
    Ok(())
}
