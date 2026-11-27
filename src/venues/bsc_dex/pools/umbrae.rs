//! Роли DAMM и DLMM Umbrae. Подтверждает конкретный рынок и обе реализации на канонической высоте.
pub(super) use super::umbrae_abi::topics;
pub(crate) use super::umbrae_abi::{IMarket, swap_topic};
pub(crate) use super::umbrae_module::{module, native};
use super::{
    families::Family,
    family_variant::Variant,
    nami_batch as batch, solidly_classic_factory as binding,
    verify::{Candidate, Verdict},
    virtuals_roles::Role,
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};
pub(crate) struct Market {
    pub tokens: [Address; 2],
    pub step: Option<u16>,
    pub router: Option<Address>,
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    at: &Value,
) -> GatewayResult<Market> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Umbrae: рынок, код, действующая реализация или реестр изменились",
        )
    };
    let generation = ctx.settings.generation();
    let bins = f.variant == Variant::UmbraeDlmm;
    let rows = ctx.dex_list("dex.umbrae_roles", f.id)?;
    let roles = rows
        .iter()
        .map(|r| Role::parse(r).ok_or_else(bad))
        .collect::<GatewayResult<Vec<_>>>()?;
    let (beacon, periphery) = roles.split_first().ok_or_else(bad)?;
    if periphery.len() != if bins { 2 } else { 0 } {
        return Err(bad());
    }
    let pool_hash = ctx
        .dex_text("dex.pair_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    let (factory, _, _, _) = tokio::try_join!(
        binding::at(ctx, f, at),
        binding::plain(ctx, pool, pool_hash, at),
        binding::plain(ctx, beacon.address, beacon.runtime, at),
        binding::plain(ctx, beacon.implementation, beacon.implementation_hash, at)
    )?;
    let slot = U256::from_be_bytes(keccak256("eip1967.proxy.beacon").0) - U256::ONE;
    let [p, b, ip, x, y] = batch::read(
        ctx,
        [
            (
                "eth_getStorageAt",
                json!([pool, B256::from(slot.to_be_bytes::<32>()), at]),
            ),
            batch::view(at, factory, IMarket::beaconCall {}),
            batch::view(at, beacon.address, IMarket::implementationCall {}),
            batch::view(at, pool, IMarket::tokenXCall {}),
            batch::view(at, pool, IMarket::tokenYCall {}),
        ],
    )
    .await?;
    let x = batch::decode(&x, IMarket::tokenXCall {})?;
    let y = batch::decode(&y, IMarket::tokenYCall {})?;
    if x >= y
        || x.is_zero()
        || super::discover::sorted(tokens.0, tokens.1) != (x, y)
        || p.as_ref() != beacon.address.into_word().as_slice()
        || batch::decode(&b, IMarket::beaconCall {})? != beacon.address
        || batch::decode(&ip, IMarket::implementationCall {})? != beacon.implementation
    {
        return Err(bad());
    }
    let (step, router) = if let [router, viewer] = periphery {
        if viewer.address != ctx.dex_addr("dex.quoter", f.id)? || router.address == viewer.address {
            return Err(bad());
        }
        tokio::try_join!(router.check(ctx, at), viewer.check(ctx, at))?;
        let c = IMarket::getPairBaseFactorCall { pair: pool };
        let [r, step, factor] = batch::read(
            ctx,
            [
                batch::view(at, router.address, IMarket::FACTORYCall {}),
                batch::view(at, pool, IMarket::binStepCall {}),
                batch::view(at, factory, c.clone()),
            ],
        )
        .await?;
        let step = batch::decode(&step, IMarket::binStepCall {})?;
        let factor = batch::decode(&factor, c)?;
        if batch::decode(&r, IMarket::FACTORYCall {})? != factory || step == 0 {
            return Err(bad());
        }
        let ab = IMarket::getPairCall {
            a: x,
            b: y,
            step,
            factor,
        };
        let ba = IMarket::getPairCall {
            a: y,
            b: x,
            step,
            factor,
        };
        let [one, two] = batch::read(
            ctx,
            [
                batch::view(at, factory, ab.clone()),
                batch::view(at, factory, ba.clone()),
            ],
        )
        .await?;
        if batch::decode(&one, ab)? != pool || batch::decode(&two, ba)? != pool {
            return Err(bad());
        }
        (Some(step), Some(router.address))
    } else {
        use super::abi_v2v3::IV2Factory;
        let ab = IV2Factory::getPairCall {
            tokenA: x,
            tokenB: y,
        };
        let ba = IV2Factory::getPairCall {
            tokenA: y,
            tokenB: x,
        };
        let [one, two, reg] = batch::read(
            ctx,
            [
                batch::view(at, factory, ab.clone()),
                batch::view(at, factory, ba.clone()),
                batch::view(at, pool, super::abi_check::IPoolImmutables::factoryCall {}),
            ],
        )
        .await?;
        if batch::decode(&one, ab)? != pool
            || batch::decode(&two, ba)? != pool
            || batch::decode(&reg, super::abi_check::IPoolImmutables::factoryCall {})? != factory
        {
            return Err(bad());
        }
        (None, None)
    };
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки Umbrae изменились во время чтения",
        ));
    }
    Ok(Market {
        tokens: [x, y],
        step,
        router,
    })
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Umbrae: требуется адрес пары".into());
    };
    let read = async {
        let at = super::nami::block(ctx, "latest").await?;
        market(ctx, c.family, pool, (c.coin, c.quote), &at).await
    };
    match read.await {
        Ok(m) => {
            let mut f = super::verify_woo::found(c, PoolRef::Contract(pool));
            [f.token0, f.token1] = m.tokens;
            f.bin_step = m.step.map(u32::from);
            Verdict::Taken(Box::new(f))
        }
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
