//! Файл: venues/bsc_dex/pools/fraxswap.rs.
//! Что делает: читает резервы Fraxswap после виртуальных ордеров к timestamp выбранного блока.
//! Кто вызывает: проверка и загрузка пулов, котировка официального роутера, шлюз перед отправкой V5.
//! Настройки: dex.factory, dex.quoter, для исполнения dex.router и dex.module_runtime_hashes.
use super::super::{
    ctx::Ctx,
    node::{call, rpc},
};
use super::{families::Family, load::Loaded};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64, U256};
use serde::Deserialize;
use serde_json::{Value, json};

sol! {
    interface IFrax {
        function factory() external view returns(address);
        function getPair(address,address) external view returns(address);
        function newSwapsPaused() external view returns(bool);
        function getTwammReserves() external view returns(uint112 r0,uint112 r1,uint32 updated,uint112 escrow0,uint112 escrow1,uint256 feeBps);
        function getReserveAfterTwamm(uint256 timestamp) external view returns(uint112 r0,uint112 r1,uint256 previousTime,uint112 escrow0,uint112 escrow1);
        function getAmountsOutWithTwamm(uint256 amountIn, address[] path) external returns(uint256[]);
    }
}

pub(crate) fn unchanged(ctx: &Ctx, generation: Option<u64>) -> GatewayResult<()> {
    if generation.is_some_and(|g| g != ctx.settings.generation()) {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Fraxswap: настройки изменились после построения шага V5",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct Header {
    hash: B256,
    timestamp: U64,
}

async fn read<C: SolCall>(
    ctx: &Ctx,
    address: Address,
    method: C,
    tag: &Value,
) -> GatewayResult<C::Return> {
    let result: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":address,"data":Bytes::from(method.abi_encode())},tag]),
    )
    .await?;
    call::decode::<C>(&result)
}

pub(super) async fn validate(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &Value,
) -> GatewayResult<()> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    let (q, p, listed) = tokio::try_join!(
        read(ctx, quoter, IFrax::factoryCall {}, tag),
        read(ctx, pool, IFrax::factoryCall {}, tag),
        read(
            ctx,
            factory,
            IFrax::getPairCall {
                _0: pair.0,
                _1: pair.1
            },
            tag
        )
    )?;
    if factory.is_zero() || listed != pool || q != factory || p != factory {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Fraxswap: пул или квотер не соответствует выбранной фабрике",
        ));
    }
    Ok(())
}

pub(crate) async fn check_adapter(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    block: &str,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let tag = super::elfomo::block(ctx, block).await?;
    let router = ctx.dex_addr("dex.router", f.id)?;
    let ((), (), actual) = tokio::try_join!(
        validate(ctx, f, pool, tokens, &tag),
        super::direct_module::verify_code(ctx, f.id, router, &tag),
        read(ctx, router, IFrax::factoryCall {}, &tag)
    )?;
    if actual != ctx.dex_addr("dex.factory", f.id)? || generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Fraxswap: привязка адаптера к фабрике изменилась",
        ));
    }
    Ok(())
}

pub(super) async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    block: &str,
) -> GatewayResult<Loaded> {
    let header: Header = rpc::call(ctx, "eth_getBlockByNumber", json!([block, false])).await?;
    let tag = json!({"blockHash":header.hash,"requireCanonical":true});
    let ((), stored, paused) = tokio::try_join!(
        validate(ctx, f, pool, tokens, &tag),
        read(ctx, pool, IFrax::getTwammReservesCall {}, &tag),
        read(ctx, pool, IFrax::newSwapsPausedCall {}, &tag)
    )?;
    let (r0, r1) = if paused {
        (stored.r0, stored.r1)
    } else {
        let projection = read(
            ctx,
            pool,
            IFrax::getReserveAfterTwammCall {
                timestamp: U256::from(header.timestamp),
            },
            &tag,
        )
        .await?;
        (projection.r0, projection.r1)
    };
    let bps: u32 = stored
        .feeBps
        .try_into()
        .ok()
        .filter(|v: &u32| *v < 10000)
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "Fraxswap: комиссия вне диапазона пары",
            )
        })?;
    Ok(Loaded {
        state: PoolState::V2 {
            reserve0: U256::from(r0),
            reserve1: U256::from(r1),
        },
        ticks: None,
        bins: None,
        fee_pips: Some(bps * 100),
    })
}
