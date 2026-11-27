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
pub(crate) fn target(ctx: &Ctx, e: &Entry) -> GatewayResult<Address> {
    let key = if e.family.variant == super::family_variant::Variant::VvsV3 { "dex.v3_native_router" } else { "dex.router" };
    ctx.dex_addr(key,e.family.id)
}
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
    use super::family_variant::Variant;
    if !matches!(e.family.variant, Variant::V2ExactRouter | Variant::RegisteredUniV3 | Variant::VvsV3)
        || !((input, output) == (e.pool.token0, e.pool.token1)
            || (output, input) == (e.pool.token0, e.pool.token1))
    {
        return Err(bad());
    }
    let router = target(ctx,e)?;
    let hash = ctx
        .dex_text("dex.router_runtime_hash", id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    tokio::try_join!(
        async {
            if e.family.variant == Variant::VvsV3 {
                return registered_v3::market(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),e.pool.fee_pips,at).await;
            }
            if e.family.variant == Variant::RegisteredUniV3 {
                return registered_v3::fresh(ctx,e,at).await;
            }
            let pool=e.pool.reference.address();
            let factory=ctx.dex_addr("dex.factory",id)?;
            let query=super::abi_v2v3::IV2Factory::getPairCall {tokenA:input,tokenB:output};
            let owner=Pool::factoryCall {};
            let ((),[listed,actual])=tokio::try_join!(
                super::v2_pair_binding::code(ctx,id,pool,at),
                batch::read(ctx,[batch::view(at,factory,query.clone()),batch::view(at,pool,owner.clone())])
            )?;
            if batch::decode(&listed,query)? != pool || batch::decode(&actual,owner)? != factory {
                return Err(bad());
            }
            Ok(())
        },
        plain(ctx, router, hash, at),
        super::exact_wallet_tokens::check(ctx, id, input, output, at)
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
