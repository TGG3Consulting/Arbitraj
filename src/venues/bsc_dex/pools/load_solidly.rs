//! Файл: venues/bsc_dex/pools/load_solidly.rs
//! Что делает: чтение состояния пула Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85) на заданном блоке:
//! `metadata()` пула (dec0, dec1, reserve0, reserve1, stable, token0, token1 — всё одним вызовом) и `getFee(pool,
//! stable)` фабрики (комиссия пула, числитель над 10000). Собирает PoolState::Solidly. Карт тиков/бинов у Solidly
//! нет (цена — из резервов и флага stable). Читается по одному пулу (как DODO/Elastic/Algebra): под пачку V2/V3
//! (load_batch.rs) не ложится — свой Pair-интерфейс.
//! Кто вызывает: pools/load.rs (ветка proto.is_solidly).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_solidly.rs, core/types (PoolState).
//! Настройки: dex.factory [DEX] (фабрика — источник комиссии getFee).
//!
//! Геттеры — velodrome-finance/contracts, HEAD b3065d8 (решение 23): Pool.metadata() (Pool.sol L122-L129),
//! PoolFactory.getFee(pool, stable) (PoolFactory.sol L131-L133). Комиссия пула читается у фабрики по флагу stable
//! пула — тем же fee, что пул применяет в своём getAmountOut (сверка квотером, quote/quoter_calls.rs). decimals0/1 из
//! metadata — это 10^decimals токена (контракт: `dec0 = 10 ** decimals0`), прямо в стабильную математику swap_solidly.

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_solidly::{ISolidlyFactory, ISolidlyPool};

/// Состояние пула Solidly на блоке. Принимает: окружение, id DEX (для адреса фабрики), адрес пула, блок-тег.
/// Ошибки: не задана dex.factory, узел, ответ не по ABI, комиссия пула не помещается в u32.
pub async fn load(ctx: &Ctx, dex_id: &str, pool: Address, tag: &str) -> GatewayResult<PoolState> {
    let meta = call::view(ctx, pool, &ISolidlyPool::metadataCall {}, tag).await?;
    let factory = ctx.dex_addr("dex.factory", dex_id)?;
    let fee_raw = call::view(
        ctx,
        factory,
        &ISolidlyFactory::getFeeCall {
            pool,
            stable: meta.st,
        },
        tag,
    )
    .await?;
    let fee = fee_u32(ctx, fee_raw)?;
    Ok(PoolState::Solidly {
        reserve0: meta.r0,
        reserve1: meta.r1,
        stable: meta.st,
        decimals0: meta.dec0,
        decimals1: meta.dec1,
        fee,
    })
}

/// Комиссия пула (числитель над 10000) из U256 getFee в u32. Ошибка BadResponse — больше делителя 10000 (комиссия
/// ≥ 100%: пул испорчен) или не помещается. Ошибок сети нет.
pub fn fee_u32(ctx: &Ctx, fee: U256) -> GatewayResult<u32> {
    let f: u32 = fee.try_into().map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Solidly getFee = {fee} не помещается"),
        )
    })?;
    if f >= 10_000 {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!("Solidly getFee = {f} ≥ 10000 (комиссия ≥ 100%)"),
        ));
    }
    Ok(f)
}
