//! Проверка официальной фабрики и квотера V3, адреса CREATE2, токенов и неизменяемого уровня комиссии.
//! Загрузчик проверяет рынок до чтения тиков. Платёжный путь также сверяет код модуля и factory().
use super::{
    abi_check::IPoolImmutables as Pool, abi_v2v3::IV3Factory as Factory, families::Family,
    nami_batch as batch, registry::Entry,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, aliases::U24, keccak256};
use serde_json::Value;

pub(crate) fn address(
    factory: Address,
    tokens: (Address, Address),
    fee: U24,
    init: B256,
) -> Address {
    factory.create2(keccak256((tokens.0, tokens.1, fee).abi_encode()), init)
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    expected: Option<u32>,
    at: &Value,
) -> GatewayResult<()> {
    let gen_before = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Не подтверждены фабрика, код, токены или комиссия рынка V3",
        )
    };
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    let hash = |k| ctx.dex_text(k, f.id)?.parse::<B256>().map_err(|_| bad());
    if ctx.dex_text("dex.v3_quoter_method", f.id)? != "quoteExactInputSingle"
        || ctx.dex_flag("dex.quoter_virtual", f.id).unwrap_or(false)
    {
        return Err(bad());
    }
    let init = super::verify_algebra_dir::hashes_for(ctx, f.id).map_err(|_| bad())?;
    tokio::try_join!(
        super::solidly_classic_factory::plain(ctx, factory, hash("dex.factory_runtime_hash")?, at),
        super::solidly_classic_factory::plain(ctx, quoter, hash("dex.quoter_runtime_hash")?, at)
    )?;
    let fa = Pool::factoryCall {};
    let t0 = Pool::token0Call {};
    let t1 = Pool::token1Call {};
    let rate = Pool::feeCall {};
    let sp = Pool::tickSpacingCall {};
    let [own, first, second, r, step, origin] = batch::read(
        ctx,
        [
            batch::view(at, pool, fa.clone()),
            batch::view(at, pool, t0.clone()),
            batch::view(at, pool, t1.clone()),
            batch::view(at, pool, rate.clone()),
            batch::view(at, pool, sp.clone()),
            batch::view(at, quoter, fa.clone()),
        ],
    )
    .await?;
    let fee = batch::decode(&r, rate)?;
    let space = batch::decode(&step, sp)?;
    if a >= b
        || a.is_zero()
        || space.as_i32() <= 0
        || fee.to::<u32>() >= 1_000_000
        || expected.is_some_and(|x| x != fee.to::<u32>())
        || batch::decode(&own, fa.clone())? != factory
        || batch::decode(&origin, fa)? != factory
        || batch::decode(&first, t0)? != a
        || batch::decode(&second, t1)? != b
        || !init
            .iter()
            .any(|h| address(factory, (a, b), fee, *h) == pool)
    {
        return Err(bad());
    }
    let one = Factory::getPoolCall {
        tokenA: a,
        tokenB: b,
        fee,
    };
    let two = Factory::getPoolCall {
        tokenA: b,
        tokenB: a,
        fee,
    };
    let tier = Factory::feeAmountTickSpacingCall { fee };
    let [ab, ba, spacing] = batch::read(
        ctx,
        [
            batch::view(at, factory, one.clone()),
            batch::view(at, factory, two.clone()),
            batch::view(at, factory, tier.clone()),
        ],
    )
    .await?;
    if batch::decode(&ab, one)? != pool
        || batch::decode(&ba, two)? != pool
        || batch::decode(&spacing, tier)? != space
        || ctx.settings.generation() != gen_before
    {
        return Err(bad());
    }
    Ok(())
}

pub(crate) async fn fresh(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<()> {
    market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        e.pool.fee_pips,
        at,
    )
    .await?;
    super::direct_module::verify_factory(
        ctx,
        e.family,
        ctx.dex_addr("dex.router", e.family.id)?,
        at,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::{
        exec::executor::kind::Params,
        pools::{exec_kind::ExecKind, families},
    };
    use serde_json::Value;

    #[test]
    fn both_factories_and_all_registered_fee_tiers_reproduce_the_archive() {
        let data: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/registered_v3_base_profiles.json"
        ))
        .unwrap();
        for (id, j) in data["markets"].as_object().unwrap() {
            let factory: Address = j["factory"].as_str().unwrap().parse().unwrap();
            let init: B256 = j["init"].as_str().unwrap().parse().unwrap();
            let pools = j["pairs"].as_array().unwrap();
            assert_eq!(pools.len(), if id == "yum-yum-swap" { 9 } else { 3 });
            for pool in pools {
                let assets = pool["tokens"].as_array().unwrap();
                let a = assets[0].as_str().unwrap().parse().unwrap();
                let b = assets[1].as_str().unwrap().parse().unwrap();
                let fee = U24::from(pool["fee"].as_u64().unwrap() as u32);
                let actual = address(factory, (a, b), fee, init);
                assert_eq!(
                    actual,
                    pool["pool"].as_str().unwrap().parse::<Address>().unwrap()
                );
                assert_ne!(actual, address(factory, (a, b), fee + U24::ONE, init));
                let mode = Params::Balancer {
                    pool_id: actual.into_word(),
                };
                let bytes = mode.data();
                assert_eq!(bytes.as_ref(), actual.into_word().as_slice());
            }
        }
        let view = families::make("registered-market", "registered_v3").unwrap();
        let cash = families::make("registered-market-module", "registered_v3_module").unwrap();
        assert!(!view.tradable && view.executor_kind.is_none());
        assert!(cash.tradable && cash.direct_module());
        assert_eq!(cash.executor_kind, Some(ExecKind::Balancer));
        assert_eq!(cash.proto, families::Proto::UniswapV3);
    }
}
