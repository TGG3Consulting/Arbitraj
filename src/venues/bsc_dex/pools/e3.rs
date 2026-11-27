//! Собственный исполняемый код E3 и учёт средств LB на Base.
use super::{abi_lb::ILBPair, dagrow, families::Family, solidly_classic_factory};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call},
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

alloy::sol! {
    interface IAccounting {
        function getReserves() external view returns(uint128 x,uint128 y);
        function getProtocolFees() external view returns(uint128 x,uint128 y);
    }
}
pub(crate) async fn roles(
    ctx: &Ctx,
    f: &Family,
    factory: Address,
    implementation: Address,
    at: &Value,
) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotConfigured,
            "E3: требуется собственная реализация LB и её подтверждённый код",
        )
    };
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", f.id)?;
    let [hash] = hashes.as_slice() else {
        return Err(bad());
    };
    let hash = hash.parse::<B256>().map_err(|_| bad())?;
    let factory_hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse()
        .map_err(|_| bad())?;
    tokio::try_join!(
        solidly_classic_factory::plain(ctx, factory, factory_hash, at),
        solidly_classic_factory::plain(ctx, implementation, hash, at)
    )?;
    Ok(())
}

pub(crate) async fn input(ctx: &Ctx, pool: Address, src: Address, at: &Value) -> GatewayResult<()> {
    let (x, y, reserves, protocol, physical) = tokio::try_join!(
        call::canonical(ctx, at, pool, ILBPair::getTokenXCall {}),
        call::canonical(ctx, at, pool, ILBPair::getTokenYCall {}),
        call::canonical(ctx, at, pool, IAccounting::getReservesCall {}),
        call::canonical(ctx, at, pool, IAccounting::getProtocolFeesCall {}),
        dagrow::balance(ctx, src, pool, at)
    )?;
    let (r, p) = if src == x {
        (reserves.x, protocol.x)
    } else if src == y {
        (reserves.y, protocol.y)
    } else {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "E3: входной токен отсутствует в паре",
        ));
    };
    if physical != U256::from(r) + U256::from(p) {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "E3: остаток на входе не совпадает с суммой резервов и протокольной комиссии",
        ));
    }
    Ok(())
}

pub(crate) async fn presend(
    ctx: &Ctx,
    legs: &[Vec<crate::venues::bsc_dex::exec::chain::Step>],
) -> GatewayResult<()> {
    let steps: Vec<_> = legs
        .iter()
        .flatten()
        .filter(|s| s.entry.family.variant.e3_lb())
        .map(|s| (s.entry.clone(), s.order.token_in))
        .collect();
    if steps.is_empty() {
        return Ok(());
    }
    let at = super::nami::block(ctx, "latest").await?;
    for (entry, token) in steps {
        super::sectorone::check(ctx, &entry, &at, false).await?;
        input(ctx, entry.pool.reference.address(), token, &at).await?;
    }
    Ok(())
}
