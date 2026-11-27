//! Штатный V3 Router проверяет номинальный минимум. Для проверенных
//! реализаций ERC20 этот минимум равен чистому приходу на кошелёк.
use super::{
    abi_check::IPoolImmutables as Pool, nami_batch as batch, registered_v3, registry::Entry,
    solidly_classic_factory::plain,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256};
use serde_json::Value;

pub(crate) use super::exact_wallet_tokens::eligible;
pub(crate) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    output: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let generation = ctx.settings.generation();
    let id = e.family.id;
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "V3 Router: не подтверждены фабрика, код или точная выплата ERC20",
        )
    };
    if !eligible(input, output)
        || e.family.variant != super::family_variant::Variant::RegisteredUniV3
        || !((input, output) == (e.pool.token0, e.pool.token1)
            || (output, input) == (e.pool.token0, e.pool.token1))
    {
        return Err(bad());
    }
    let router = ctx.dex_addr("dex.router", id)?;
    let hash = ctx
        .dex_text("dex.router_runtime_hash", id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    tokio::try_join!(
        registered_v3::fresh(ctx, e, at),
        plain(ctx, router, hash, at),
        super::exact_wallet_tokens::check(ctx, input, output, at)
    )?;
    let f = Pool::factoryCall {};
    let [actual] = batch::read(ctx, [batch::view(at, router, f.clone())]).await?;
    if batch::decode(&actual, f)? != ctx.dex_addr("dex.factory", id)?
        || ctx.settings.generation() != generation
    {
        return Err(bad());
    }
    Ok(router)
}
