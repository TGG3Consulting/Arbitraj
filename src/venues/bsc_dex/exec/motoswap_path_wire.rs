//! Проверка нескольких пар Motoswap и подготовка единого вызова FeeRouter или модуля.
//! Планировщик маршрута передаёт последовательность валют и пар; роли и версию модуля проверяем на одном блоке.
use super::{bad, shape, terms};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::{
    ctx::Ctx, exec::motoswap_wallet::{Route, abi::IModule}, node::call::canonical,
    pools::{motoswap, registry::Entry}, quote::v2_wallet_sim as sim,
}};
use alloy_primitives::{Address, U256};
use serde_json::Value;

pub(in crate::venues::bsc_dex) async fn path_wire(
    ctx: &Ctx,
    entries: &[&Entry],
    target: Address,
    path: Vec<Address>,
    at: &Value,
) -> GatewayResult<(sim::Wire, Vec<crate::core::types::MotoswapState>)> {
    let first = *entries.first().ok_or_else(|| bad(ctx))?;
    if entries.len() < 2 || entries.len() + 1 != path.len() || path.first() == path.last() {
        return Err(bad(ctx));
    }
    let mut pools = Vec::with_capacity(entries.len());
    for (e, pair) in entries.iter().zip(path.windows(2)) {
        let pool = shape(e).ok_or_else(|| bad(ctx))?;
        if e.family.id != first.family.id
            || pools.contains(&pool)
            || (e.pool.token0, e.pool.token1) != (pair[0].min(pair[1]), pair[0].max(pair[1]))
        {
            return Err(bad(ctx));
        }
        pools.push(pool);
    }
    let checks = entries
        .iter()
        .map(|e| {
            (
                e.family,
                e.pool.reference.address(),
                (e.pool.token0, e.pool.token1),
            )
        })
        .collect::<Vec<_>>();
    let checked = futures_util::future::try_join_all(
        checks
            .into_iter()
            .map(|(family, pool, pair)| motoswap::state(ctx, family, pool, pair, at)),
    );
    let (t, states) = tokio::try_join!(terms(ctx, first, path[0], target, Some(at)), checked)?;
    if t.module_hash.is_some()
        && canonical(ctx, at, target, IModule::pathVersionCall {}).await? != U256::ONE
    {
        return Err(bad(ctx));
    }
    Ok((
        sim::Wire::MotoswapPath(
            t,
            std::sync::Arc::new(Route { pools, path }),
        ),
        states,
    ))
}
