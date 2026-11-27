//! Блоковое состояние DeltaSwap и привязка нативного Router02 к выбранному пулу.
use super::{families::Family, load::Loaded, nami_batch as batch, registry::Entry};
use crate::core::types::{PoolState, pool::DeltaFee};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;

sol! {
    interface IDelta {
        function factory() external view returns(address);
        function WETH() external view returns(address);
        function getPair(address,address) external view returns(address);
        function getReserves() external view returns(uint112 r0,uint112 r1,uint32 stamp);
        function getLiquidityEMA() external view returns(uint112 value,uint32 updated);
        function getTradeLiquidityEMAParams() external view returns(uint112 traded,uint112 sum,uint32 updated);
        function gammaPool() external view returns(address);
        function dsFeeInfo() external view returns(uint8 rate,uint8 threshold);
    }
}

pub(super) async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    block: u64,
) -> GatewayResult<Loaded> {
    let tag = super::elfomo::block(ctx, &format!("0x{block:x}")).await?;
    load_at(ctx, f, pool, pair, block, tag).await
}

async fn load_at(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    block: u64,
    tag: serde_json::Value,
) -> GatewayResult<Loaded> {
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let (router, expected_hash) =
        crate::venues::bsc_dex::quote::router_pricing::profile(ctx, f.id)?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let invalid = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "DeltaSwap: размещение, каноническая пара или состояние комиссии расходятся с профилем",
        )
    };
    let inits = super::verify_algebra_dir::hashes_for(ctx, f.id)
        .map_err(|e| ctx.err(ErrorCode::NotApplicable, e))?;
    if router == factory
        || pair.0 >= pair.1
        || !inits
            .iter()
            .any(|h| super::verify::v2_create2(factory, pair.0, pair.1, *h) == pool)
    {
        return Err(invalid());
    }
    let registered = IDelta::getPairCall {
        _0: pair.0,
        _1: pair.1,
    };
    let [
        code,
        of_router,
        wrapped_router,
        of_pool,
        present,
        reserves,
        liquidity,
        trades,
        gamma,
        fees,
    ] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([router, tag])),
            batch::view(&tag, router, IDelta::factoryCall {}),
            batch::view(&tag, router, IDelta::WETHCall {}),
            batch::view(&tag, pool, IDelta::factoryCall {}),
            (
                "eth_call",
                json!([{"to":factory,"data":Bytes::from(registered.abi_encode())},tag]),
            ),
            batch::view(&tag, pool, IDelta::getReservesCall {}),
            batch::view(&tag, pool, IDelta::getLiquidityEMACall {}),
            batch::view(&tag, pool, IDelta::getTradeLiquidityEMAParamsCall {}),
            batch::view(&tag, pool, IDelta::gammaPoolCall {}),
            batch::view(&tag, factory, IDelta::dsFeeInfoCall {}),
        ],
    )
    .await?;
    if code.is_empty()
        || alloy_primitives::keccak256(&code) != expected_hash
        || batch::decode(&of_router, IDelta::factoryCall {})? != factory
        || batch::decode(&of_pool, IDelta::factoryCall {})? != factory
        || batch::decode(&wrapped_router, IDelta::WETHCall {})? != wrapped
        || batch::decode(&present, registered)? != pool
        || batch::decode(&gamma, IDelta::gammaPoolCall {})? == router
        || generation != ctx.settings.generation()
    {
        return Err(invalid());
    }
    let r = batch::decode(&reserves, IDelta::getReservesCall {})?;
    let l = batch::decode(&liquidity, IDelta::getLiquidityEMACall {})?;
    let t = batch::decode(&trades, IDelta::getTradeLiquidityEMAParamsCall {})?;
    let f = batch::decode(&fees, IDelta::dsFeeInfoCall {})?;
    if u64::from(l.updated) > block || u64::from(t.updated) > block {
        return Err(invalid());
    }
    let fee = Box::new(DeltaFee {
        router,
        generation,
        block,
        liquidity: U256::from(l.value),
        liquidity_block: l.updated,
        traded: U256::from(t.traded),
        sum: U256::from(t.sum),
        trade_block: t.updated,
        rate: f.rate,
        threshold: f.threshold,
    });
    Ok(Loaded {
        state: PoolState::DeltaSwap {
            reserve0: U256::from(r.r0),
            reserve1: U256::from(r.r1),
            fee,
        },
        ticks: None,
        bins: None,
        fee_pips: None,
    })
}

/// Адрес получателя разрешения совпадает с адресом, чей код и фабрика проверены при загрузке состояния.
pub(crate) fn router(ctx: &Ctx, e: &Entry) -> GatewayResult<Address> {
    let key=if e.family.variant.empire_module() {"dex.quoter"} else {"dex.router"};
    let router = ctx.dex_addr(key, e.family.id)?;
    if e.family.variant == super::family_variant::Variant::DeltaSwap
        && !matches!(&e.pool.state,PoolState::DeltaSwap{fee,..} if fee.router==router && fee.generation==ctx.settings.generation())
    {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "DeltaSwap: состояние рынка или настройки изменились до подготовки обмена",
        ));
    }
    Ok(router)
}

pub(crate) async fn refresh(
    ctx: &Ctx,
    e: &mut Entry,
    head: crate::venues::bsc_dex::quote::white_state::Head,
) -> GatewayResult<()> {
    let block = head.number.to();
    let loaded = load_at(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        block,
        head.selector(),
    )
    .await?;
    e.pool.state = loaded.state;
    e.pool.block = block;
    e.pool.fee_pips = None;
    Ok(())
}
