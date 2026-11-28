//! Канонические роли собственной pull-кассы Stelaxis. Цена меняется уже в слоте строителя.
use super::{Ctx, Family, Roles, bad, batch};
use crate::gateway::errors::GatewayResult;
use crate::venues::bsc_dex::exec::prop_abi::{IStelaxis, IStrict};
use alloy_primitives::{Address, B256};
use serde_json::Value;

pub(super) async fn listed(
    ctx: &Ctx,
    r: &Roles,
    a: Address,
    b: Address,
    at: &Value,
) -> GatewayResult<bool> {
    let (reply, (), (), ()) = tokio::try_join!(
        batch::read(
            ctx,
            [batch::view(at, r.venue.address, IStelaxis::getPairsCall {})]
        ),
        r.venue.check(ctx, at),
        r.vault.check(ctx, at),
        r.fallback.check(ctx, at)
    )?;
    let list = batch::decode(&reply[0], IStelaxis::getPairsCall {})?;
    Ok(list
        .iter()
        .any(|p| (p.token0, p.token1) == (a, b) || (p.token0, p.token1) == (b, a)))
}
pub(super) async fn module(
    ctx: &Ctx,
    f: &Family,
    r: &Roles,
    m: Address,
    at: &Value,
) -> GatewayResult<B256> {
    let (row, hash) = tokio::try_join!(
        batch::read(
            ctx,
            [
                batch::view(at, m, IStrict::marketCall {}),
                batch::view(at, m, IStelaxis::registryCall {}),
                batch::view(at, m, IStrict::vaultCall {}),
                batch::view(at, m, IStrict::marketHashCall {}),
                batch::view(at, m, IStelaxis::registryHashCall {}),
                batch::view(at, m, IStrict::vaultHashCall {})
            ]
        ),
        super::super::direct_module::verified_hash(ctx, f.id, m, at)
    )?;
    if [
        batch::decode(&row[0], IStrict::marketCall {})?,
        batch::decode(&row[1], IStelaxis::registryCall {})?,
        batch::decode(&row[2], IStrict::vaultCall {})?,
    ] != [r.venue.address, r.fallback.address, r.vault.address]
        || [
            batch::decode(&row[3], IStrict::marketHashCall {})?,
            batch::decode(&row[4], IStelaxis::registryHashCall {})?,
            batch::decode(&row[5], IStrict::vaultHashCall {})?,
        ] != [r.venue.hash, r.fallback.hash, r.vault.hash]
    {
        return Err(bad(ctx));
    }
    Ok(hash)
}
