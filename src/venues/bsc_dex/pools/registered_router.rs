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
/// Дополнительный модуль для стандартного callback. Штатный V5 остаётся доступен при пустом адресе.
pub(crate) fn wallet_module(ctx:&Ctx,e:&Entry)->Option<Address>{
    (matches!(e.family.variant,super::family_variant::Variant::RegisteredUniV3 | super::family_variant::Variant::TheoremIntegral)).then(||ctx.dex_addr("dex.v3_wallet_module",e.family.id).ok()).flatten().filter(|a|!a.is_zero())
}
pub(crate) async fn checked_module(ctx:&Ctx,e:&Entry,at:&Value)->GatewayResult<Address>{
    Ok(module_binding(ctx,e,at).await?.0)
}
pub(crate) async fn module_binding(ctx:&Ctx,e:&Entry,at:&Value)->GatewayResult<(Address,B256)>{
    let id=e.family.id;let generation=ctx.settings.generation();
    let bad=||ctx.err(ErrorCode::NoRoute,"Модуль V3, его фабрика или доступная сборка изменились");
    let module=wallet_module(ctx,e).ok_or_else(bad)?;
    let accepted=super::direct_module::configured(ctx,id)?;
    let fa=Pool::factoryCall{};
    let bind_market=async {
        if e.family.variant == super::family_variant::Variant::TheoremIntegral {
            return Box::pin(super::theorem::wallet(ctx,e,module,at)).await;
        }
        registered_v3::market(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),e.pool.fee_pips,at).await
    };
    let (_,vals)=tokio::try_join!(bind_market,
       batch::read(ctx,[("eth_getCode",serde_json::json!([module,at])),batch::view(at,module,fa.clone())]))?;
    let hash=super::direct_module::matching_hash(&accepted,&vals[0]).ok_or_else(bad)?;
    if batch::decode(&vals[1],fa)?!=ctx.dex_addr("dex.factory",id)? || ctx.settings.generation()!=generation{return Err(bad())}
    Ok((module,hash))
}
pub(crate) fn target(ctx: &Ctx, e: &Entry) -> GatewayResult<Address> {
    let key = if e.family.variant == super::family_variant::Variant::SlipstreamUniversal { "dex.universal_router" } else if e.family.variant.separate_v3_router() { "dex.v3_native_router" } else { "dex.router" };
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
    if e.family.variant == Variant::MaverickTwoRouter {return Box::pin(super::maverick_two_router::checked(ctx,e,input,output,at)).await;}
    if e.family.variant == Variant::SlipstreamUniversal {return Box::pin(super::slipstream_ur::checked(ctx,e,input,output,at)).await;}
    if !e.family.variant.exact_router() || matches!(e.family.variant, Variant::SolidlyClone | Variant::SolidlyProxy | Variant::SolidlyPairOverride)
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
            if e.family.variant.treble_spot() {return Box::pin(super::treble_spot::router(ctx,e,router,at)).await;}
            if e.family.variant.caller_algebra(){return Box::pin(super::cypher::router(ctx,e,router,at)).await;}
            if e.family.variant.separate_v3_router() {
                return registered_v3::market(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),e.pool.fee_pips,at).await;
            }
            if e.family.variant.registered_slipstream() {
                if ctx.dex_text("dex.v3_wallet_router_abi",id).ok().as_deref()!=Some("classic_spacing"){return Err(bad())}
                let now=Box::pin(super::topaz::fee(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),e.pool.tick_spacing.ok_or_else(bad)?,at)).await?;
                return (Some(now)==e.pool.fee_pips).then_some(()).ok_or_else(bad);
            }
            if e.family.variant.spacing_keyed() && ctx.dex_text("dex.v3_wallet_router_abi",id).ok().as_deref()!=Some("router02_spacing") {return Err(bad())}
            if matches!(e.family.variant, Variant::RegisteredUniV3|Variant::RegisteredSpacingV3) {
                return registered_v3::fresh(ctx,e,at).await;
            }
            if e.family.variant.empire_nominal() {return Box::pin(super::empire::fresh(ctx,e,at)).await;}
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
