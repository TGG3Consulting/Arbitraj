//! Файл: venues/bsc_dex/pools/discover_v4.rs
//! Что делает: поиск пулов V4 по событиям Initialize менеджера (Infinity CL, Infinity Bin, Uniswap V4):
//! из лога собирается PoolKey, PoolId считается заново и сверяется с topics[1] (защита от ошибки ABI),
//! по флагам считается группа крючка. Читаются все Initialize менеджера одним проходом (фильтр только topic0):
//! встречный токен (USDT или промежуточная монета) выбирается уже из прочитанного (discover.rs) — так история
//! не читается заново на каждый встречный токен.
//! Кто вызывает: pools/discover.rs, pools/history_v4.rs, pools/history_store.rs, pools/created.rs, pools/verify_v4.rs (from_key).
//! От чего зависит: ctx.rs, pools/scan.rs, abi_v4.rs, key.rs, hooks.rs, discover.rs (Found).
//! Настройки: через scan.rs — dex.logs_block_batch, dex.scan_progress_s.
//!
//! Initialize и порядок полей — research/flow/bsc_v4.md, раздел 3 (ICLPoolManager.sol#L37-L46,
//! IBinPoolManager.sol#L52-L60, Uniswap IPoolManager.sol#L60-L69). Реестра пулов у менеджеров нет — только события
//! («единственный источник списка пулов V4 — логи Initialize»).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};
use serde_json::json;

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

/// topic0 события Initialize вида пула (фильтр eth_getLogs и подписки). Ошибок нет.
pub fn topic(f: &Family) -> B256 {
    match f.proto {
        Proto::InfinityCl => IInfinityCl::Initialize::SIGNATURE_HASH,
        Proto::InfinityBin => IInfinityBin::Initialize::SIGNATURE_HASH,
        _ => IUniswapV4::Initialize::SIGNATURE_HASH,
    }
}

/// Все события Initialize менеджера за блоки [from, to] (любые валюты) — для истории (history_v4.rs, discover.rs).
/// Принимает: вид пула, адрес менеджера, блоки. Возвращает: логи как есть (их хранит history_store.rs).
/// Ошибки: NotConfigured (dex.logs_block_batch, dex.scan_progress_s), узел.
pub async fn logs(
    ctx: &Ctx,
    f: &'static Family,
    manager: Address,
    from: u64,
    to: u64,
) -> GatewayResult<Vec<Log>> {
    let what = format!("{}: Initialize", f.title);
    scan::logs(ctx, manager, json!([topic(f)]), from, to, &what).await
}

/// Разбор пачки логов Initialize. Ошибки: как у parse (первый же неверный лог — ошибка целиком).
pub fn parse_all(ctx: &Ctx, f: &'static Family, logs: &[Log]) -> GatewayResult<Vec<Found>> {
    logs.iter().map(|log| parse(ctx, f, log)).collect()
}

/// Разбор одного Initialize в найденный пул. coin = currency0 до выбора встречного токена (discover::against).
/// Ошибки: BadResponse — событие не по ABI или PoolId из ключа не равен PoolId события.
pub fn parse(ctx: &Ctx, f: &'static Family, log: &Log) -> GatewayResult<Found> {
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
    Ok(from_key(f, manager, id, key, spacing, step))
}

/// Найденный пул V4 по ключу (из события Initialize или из ключа у менеджера / PositionManager — pools/verify_v4.rs).
/// Принимает: вид, менеджер, PoolId (уже сверенный с ключом), ключ, tickSpacing (CL), binStep (Bin).
/// Возвращает: Found; coin = currency0 до выбора встречного токена (discover::against). Ошибок нет.
pub fn from_key(
    f: &'static Family,
    manager: Address,
    id: B256,
    key: PoolKey,
    spacing: Option<i32>,
    step: Option<u32>,
) -> Found {
    let (token0, token1) = match &key {
        PoolKey::Infinity(k) => (k.currency0, k.currency1),
        PoolKey::Uniswap(k) => (k.currency0, k.currency1),
    };
    Found {
        family: f,
        reference: PoolRef::Singleton { manager, id },
        token0,
        token1,
        coin: token0,
        fee_pips: Some(key.fee()),
        tick_spacing: spacing,
        bin_step: step,
        hooks: Some(hooks::info(&key)),
        key: Some(key),
    }
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
