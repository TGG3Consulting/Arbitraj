//! Файл: venues/bsc_dex/pools/discover_v4.rs
//! Что делает: поиск пулов V4 с USDT по событиям Initialize менеджера (Infinity CL, Infinity Bin, Uniswap V4):
//! из лога собирается PoolKey, PoolId считается заново и сверяется с topics[1] (защита от ошибки ABI),
//! по флагам считается группа крючка.
//! Кто вызывает: pools/discover.rs.
//! От чего зависит: ctx.rs, pools/scan.rs, abi_v4.rs, key.rs, hooks.rs, discover.rs (Found).
//! Настройки: dex.pool_manager [вид пула V4], dex.scan_from_block, dex.logs_block_batch.
//!
//! Initialize и порядок полей — research/flow/bsc_v4.md, раздел 3 (ICLPoolManager.sol#L37-L46,
//! IBinPoolManager.sol#L52-L60, Uniswap IPoolManager.sol#L60-L69); USDT может быть currency0 или currency1 —
//! два фильтра: [topic0, null, USDT] и [topic0, null, null, USDT]. Реестра пулов у менеджеров нет — только события.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::abi_v4::{IInfinityBin, IInfinityCl, IUniswapV4};
use super::discover::Found;
use super::families::{Family, Proto};
use super::key::{
    InfinityPoolKeyData, PoolKey, UniswapPoolKeyData, infinity_bin_step, infinity_tick_spacing,
};
use super::{hooks, scan};

/// Все пулы вида V4 с USDT за блоки [from, to].
/// Ошибки: NotConfigured (dex.pool_manager, dex.logs_block_batch), узел, BadResponse (событие не по ABI,
/// PoolId не сошёлся).
pub async fn scan(
    ctx: &Ctx,
    f: &'static Family,
    usdt: Address,
    from: u64,
    to: u64,
) -> GatewayResult<Vec<Found>> {
    let manager = ctx.dex_addr("dex.pool_manager", f.id)?;
    let t0 = match f.proto {
        Proto::InfinityCl => IInfinityCl::Initialize::SIGNATURE_HASH,
        Proto::InfinityBin => IInfinityBin::Initialize::SIGNATURE_HASH,
        _ => IUniswapV4::Initialize::SIGNATURE_HASH,
    };
    let u = B256::left_padding_from(usdt.as_slice());
    let mut out = Vec::new();
    for topics in [
        json!([t0, Value::Null, u]),
        json!([t0, Value::Null, Value::Null, u]),
    ] {
        for log in scan::logs(
            ctx,
            manager,
            topics,
            from,
            to,
            &format!("{}: Initialize", f.title),
        )
        .await?
        {
            out.push(parse(ctx, f, &log, usdt)?);
        }
    }
    Ok(out)
}

/// Разбор одного Initialize в найденный пул.
/// Ошибки: BadResponse — событие не по ABI или PoolId из ключа не равен PoolId события.
pub fn parse(ctx: &Ctx, f: &'static Family, log: &Log, usdt: Address) -> GatewayResult<Found> {
    let bad = |e: String| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{}: Initialize: {e}", f.title),
        )
    };
    let manager = log.inner.address;
    let (id, key, spacing, step) = match f.proto {
        Proto::InfinityCl => {
            let e = IInfinityCl::Initialize::decode_log_data(&log.inner.data)
                .map_err(|e| bad(e.to_string()))?;
            let k = InfinityPoolKeyData {
                currency0: e.currency0,
                currency1: e.currency1,
                hooks: e.hooks,
                manager,
                fee: e.fee.to(),
                parameters: e.parameters,
            };
            (
                e.id,
                PoolKey::Infinity(k),
                Some(infinity_tick_spacing(e.parameters)),
                None,
            )
        }
        Proto::InfinityBin => {
            let e = IInfinityBin::Initialize::decode_log_data(&log.inner.data)
                .map_err(|e| bad(e.to_string()))?;
            let k = InfinityPoolKeyData {
                currency0: e.currency0,
                currency1: e.currency1,
                hooks: e.hooks,
                manager,
                fee: e.fee.to(),
                parameters: e.parameters,
            };
            (
                e.id,
                PoolKey::Infinity(k),
                None,
                Some(infinity_bin_step(e.parameters)),
            )
        }
        _ => {
            let e = IUniswapV4::Initialize::decode_log_data(&log.inner.data)
                .map_err(|e| bad(e.to_string()))?;
            let spacing = e.tickSpacing.as_i32();
            let k = UniswapPoolKeyData {
                currency0: e.currency0,
                currency1: e.currency1,
                fee: e.fee.to(),
                tick_spacing: e.tickSpacing,
                hooks: e.hooks,
            };
            (e.id, PoolKey::Uniswap(k), Some(spacing), None)
        }
    };
    if key.id() != id {
        return Err(mismatch(ctx, f, id, key.id()));
    }
    let (token0, token1) = match &key {
        PoolKey::Infinity(k) => (k.currency0, k.currency1),
        PoolKey::Uniswap(k) => (k.currency0, k.currency1),
    };
    Ok(Found {
        family: f,
        reference: PoolRef::Singleton { manager, id },
        token0,
        token1,
        coin: if token0 == usdt { token1 } else { token0 },
        fee_pips: Some(key.fee()),
        tick_spacing: spacing,
        bin_step: step,
        hooks: Some(hooks::info(&key)),
        key: Some(key),
    })
}

/// Ошибка «PoolId из ключа не совпал с PoolId события».
fn mismatch(ctx: &Ctx, f: &Family, event: B256, ours: B256) -> GatewayError {
    ctx.err(
        ErrorCode::BadResponse,
        format!(
            "{}: PoolId события {event} ≠ посчитанного по ключу {ours} — ABI не совпал",
            f.title
        ),
    )
}
