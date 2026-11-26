//! Файл: venues/bsc_dex/pools/load_solidly.rs
//! Что делает: чтение состояния пула Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85) на заданном блоке:
//! `metadata()` пула (dec0, dec1, reserve0, reserve1, stable, token0, token1 — всё одним вызовом) и `getFee(pool,
//! stable)` фабрики (комиссия пула, числитель над 10000). Собирает PoolState::Solidly. Карт тиков/бинов у Solidly
//! нет (цена — из резервов и флага stable). Читается по одному пулу (как DODO/Elastic/Algebra): под пачку V2/V3
//! (load_batch.rs) не ложится — свой Pair-интерфейс.
//! Решение 273: задана dex.solidly_fee_source — комиссия по этому источнику со своим правилом (числитель над
//! делителем / делитель) и ограничения обмена с фабрики (пауза, предел цены газа) — pools/solidly_fee_read.rs. Не задана
//! — комиссия по виду пула, как раньше (правило «числитель над 10000»); ограничения — только если заданы их методы.
//! Кто вызывает: pools/load.rs (ветка proto.is_solidly).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_solidly.rs, pools/solidly_fee_read.rs, math/solidly_math.rs
//! (fee_denominator), core/types (PoolState, SolidlyFee).
//! Настройки: dex.factory [DEX] (фабрика — источник комиссии getFee); через solidly_fee_read.rs — dex.solidly_fee_*,
//! dex.solidly_pause_method, dex.solidly_max_gas_method [DEX].
//! Solidly V1 — THENA V1 (решение 122 F): комиссия — `getFee(bool stable)` фабрики (без адреса пула;
//! ThenafiBNB/THENA-Contracts HEAD f5bc742, PairFactoryUpgradeable.sol L104-L106; её же применяет Pair.getAmountOut
//! L461). Остальное чтение (metadata) — то же.
//!
//! Геттеры — velodrome-finance/contracts, HEAD b3065d8 (решение 23): Pool.metadata() (Pool.sol L122-L129),
//! PoolFactory.getFee(pool, stable) (PoolFactory.sol L131-L133). Комиссия пула читается у фабрики по флагу stable
//! пула — тем же fee, что пул применяет в своём getAmountOut (сверка квотером, quote/quoter_calls.rs). decimals0/1 из
//! metadata — это 10^decimals токена (контракт: `dec0 = 10 ** decimals0`), прямо в стабильную математику swap_solidly.

use alloy_primitives::{Address, U256};

use crate::core::types::{FeeRule, PoolState, SolidlyFee};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::solidly_math::fee_denominator;
use super::super::node::call;
use super::abi_solidly::{ISolidlyFactory, ISolidlyPool, IThenaV1Factory};
use super::families::{Family, Proto};
use super::solidly_fee_read;

/// Состояние пула Solidly на блоке. Принимает: окружение, вид пула (id DEX — адрес фабрики; протокол — какой getFee),
/// адрес пула, блок-тег. Ошибки: не задана dex.factory, узел, ответ не по ABI, комиссия пула не помещается в u32;
/// решение 273 — не задан метод/правило/делитель заданного источника, комиссия ≥ 100% по правилу.
pub async fn load(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<PoolState> {
    if f.variant == super::family_variant::Variant::LeetSwap {
        return super::leetswap::load(ctx, f, pool, tag).await;
    }
    let meta = call::view(ctx, pool, &ISolidlyPool::metadataCall {}, tag).await?;
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let fee = pool_fee(ctx, f, (factory, pool, meta.st), tag).await?;
    let gate = solidly_fee_read::gate(ctx, f, factory, tag).await?;
    Ok(PoolState::Solidly {
        reserve0: meta.r0,
        reserve1: meta.r1,
        stable: meta.st,
        decimals0: meta.dec0,
        decimals1: meta.dec1,
        fee,
        gate,
        fee1: None,
    })
}

/// Комиссия пула с правилом (решение 273): задан источник dex.solidly_fee_source — по нему (solidly_fee_read.rs);
/// нет — по виду пула, как до решения 273 (getFee(stable) у Solidly V1, getFee(pool, stable) у Solidly; правило
/// «числитель над 10000»). Принимает: окружение, вид, (фабрика, пул, stable), блок. Ошибки: как load.
pub async fn pool_fee(
    ctx: &Ctx,
    f: &Family,
    (factory, pool, stable): (Address, Address, bool),
    tag: &str,
) -> GatewayResult<SolidlyFee> {
    if let Some(src) = solidly_fee_read::source(ctx, f)? {
        return solidly_fee_read::fee(ctx, f, &src, (factory, pool, stable), tag).await;
    }
    let fee_raw = if f.proto == Proto::SolidlyV1 {
        let ask = IThenaV1Factory::getFeeCall { stable };
        call::view(ctx, factory, &ask, tag).await?
    } else {
        let ask = ISolidlyFactory::getFeeCall { pool, stable };
        call::view(ctx, factory, &ask, tag).await?
    };
    Ok(SolidlyFee {
        value: fee_u32(ctx, fee_raw)?,
        rule: FeeRule::Numerator {
            denominator: fee_denominator().to::<u32>(),
        },
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
