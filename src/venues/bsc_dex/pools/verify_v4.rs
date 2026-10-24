//! Файл: venues/bsc_dex/pools/verify_v4.rs
//! Что делает: проверка пула V4 из справочника индексатора (у него адрес пула — PoolId) на своём узле, одним проходом
//! Multicall3: PancakeSwap Infinity — ключ пула у своего менеджера (poolIdToPoolKey), Uniswap V4 — у PositionManager
//! (poolKeys по первым 25 байтам PoolId; сам PositionManager сверяется с менеджером — poolManager()). Ключ должен дать
//! тот же PoolId (keccak256 ключа), менеджер в ключе — менеджер DEX из настроек, валюты — монета и встречный токен.
//! Так история событий Initialize (dex.history_scan) для этих пулов не нужна.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_check.rs, key.rs, discover_v4.rs (from_key), discover.rs.
//! Настройки: dex.pool_manager, dex.position_manager [DEX].
//!
//! ABI и ссылки — abi_check.rs; PoolKey и PoolId — key.rs (research/flow/bsc_v4.md, раздел 2).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, FixedBytes};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_check::{IInfinityKeys, IPositionManagerKeys};
use super::discover::{Found, sorted};
use super::discover_v4::from_key;
use super::families::Proto;
use super::key::{
    InfinityPoolKeyData, PoolKey, UniswapPoolKeyData, infinity_bin_step, infinity_tick_spacing,
};
use super::verify::{Candidate, Verdict};

/// Проверка пачки кандидатов V4 частями по `size` вызовов. Возвращает: итог по каждому (сбой узла на части — Failed).
/// Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<(B256, Address, Address), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = (|| {
            let id = B256::from_str(&c.pool).map_err(|_| format!("«{}» — не PoolId", c.pool))?;
            let manager = ctx
                .dex_addr("dex.pool_manager", c.family.id)
                .map_err(|e| e.message)?;
            let (to, data) = match c.family.proto {
                Proto::UniswapV4 => {
                    let posm = ctx
                        .dex_addr("dex.position_manager", c.family.id)
                        .map_err(|e| e.message)?;
                    let short = FixedBytes::<25>::from_slice(&id[..25]);
                    (
                        posm,
                        IPositionManagerKeys::poolKeysCall { poolId: short }.abi_encode(),
                    )
                }
                _ => (
                    manager,
                    IInfinityKeys::poolIdToPoolKeyCall { id }.abi_encode(),
                ),
            };
            Ok((id, manager, to, Bytes::from(data)))
        })();
        match plan {
            Ok((id, manager, to, data)) => {
                calls.push((to, data));
                // Uniswap V4: PositionManager должен работать с этим менеджером.
                if c.family.proto == Proto::UniswapV4 {
                    calls.push((
                        to,
                        Bytes::from(IPositionManagerKeys::poolManagerCall {}.abi_encode()),
                    ));
                }
                plans.push(Ok((id, manager, to)));
            }
            Err(w) => plans.push(Err(w)),
        }
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    let none = || Err("нет ответа".to_string());
    for (c, plan) in cands.iter().zip(plans) {
        let Ok((id, manager, _)) = plan else {
            out.push(Verdict::Rejected(plan.err().unwrap_or_default()));
            continue;
        };
        let key = got.next().unwrap_or_else(none);
        let pm = match c.family.proto {
            Proto::UniswapV4 => got.next().unwrap_or_else(none),
            _ => Ok(Err("не нужен".into())),
        };
        out.push(match (key, pm) {
            (Err(node), _) | (_, Err(node)) => Verdict::Failed(node),
            (Ok(key), Ok(pm)) => match decide(c, id, manager, key, pm) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Решение по ответам (без сети). Принимает: кандидата, PoolId, менеджер из настроек, ответ ключа, ответ poolManager()
/// (нужен только Uniswap V4). Возвращает: найденный пул или причину отказа.
pub fn decide(
    c: &Candidate,
    id: B256,
    manager: Address,
    key: Result<Bytes, String>,
    pm: Result<Bytes, String>,
) -> Result<Found, String> {
    let f = c.family;
    let key = key.map_err(|w| format!("ключ пула не прочитан: {w}"))?;
    let dec = |e: alloy::sol_types::Error| format!("ответ не по ABI: {e}");
    let (pk, spacing, step) = match f.proto {
        Proto::UniswapV4 => {
            let pm = pm.map_err(|w| format!("poolManager() PositionManager: {w}"))?;
            let pm = IPositionManagerKeys::poolManagerCall::abi_decode_returns(&pm).map_err(dec)?;
            if pm != manager {
                return Err(format!(
                    "PositionManager работает с менеджером {pm}, у DEX {} — {manager} (dex.pool_manager)",
                    f.id
                ));
            }
            let k = IPositionManagerKeys::poolKeysCall::abi_decode_returns(&key).map_err(dec)?;
            if k.tickSpacing.is_zero() {
                return Err("ключа пула нет у PositionManager (нет позиций через него) — найти можно только историей V4 (dex.history_scan)".into());
            }
            let spacing = k.tickSpacing.as_i32();
            let data = UniswapPoolKeyData {
                currency0: k.currency0,
                currency1: k.currency1,
                fee: k.fee.to(),
                tick_spacing: k.tickSpacing,
                hooks: k.hooks,
            };
            (PoolKey::Uniswap(data), Some(spacing), None)
        }
        _ => {
            let k = IInfinityKeys::poolIdToPoolKeyCall::abi_decode_returns(&key).map_err(dec)?;
            if k.poolManager != manager {
                return Err(format!(
                    "менеджер не знает этот PoolId (в ключе менеджер {}, у DEX {} — {manager})",
                    k.poolManager, f.id
                ));
            }
            let (spacing, step) = match f.proto {
                Proto::InfinityBin => (None, Some(infinity_bin_step(k.parameters))),
                _ => (Some(infinity_tick_spacing(k.parameters)), None),
            };
            let data = InfinityPoolKeyData {
                currency0: k.currency0,
                currency1: k.currency1,
                hooks: k.hooks,
                manager: k.poolManager,
                fee: k.fee.to(),
                parameters: k.parameters,
            };
            (PoolKey::Infinity(data), spacing, step)
        }
    };
    if pk.id() != id {
        return Err(format!("ключ пула даёт PoolId {}, а не {id}", pk.id()));
    }
    let found = from_key(f, manager, id, pk, spacing, step);
    if (found.token0, found.token1) != sorted(c.coin, c.quote) {
        return Err(format!(
            "валюты пула {} / {} ≠ монета {} и встречный {} (адрес монеты — от биржи)",
            found.token0, found.token1, c.coin, c.quote
        ));
    }
    Ok(Found {
        coin: c.coin,
        ..found
    })
}
