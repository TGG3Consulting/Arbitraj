//! Файл: venues/bsc_dex/pools/verify_inf_stable.rs
//! Что делает: проверка пула PancakeSwap Infinity Stable (решение «122 (Infinity Stable)») и сборка найденного пула из
//! ключа PoolKey. Кандидат из справочника или события — PoolId; ключ берётся у своего менеджера одним проходом
//! Multicall3 (poolIdToPoolKey), затем: ключ даёт тот же PoolId, менеджер в ключе — dex.pool_manager, крючок ключа — в
//! списке dex.stable_hooks этой DEX, в parameters включены beforeSwap и beforeSwapReturnsDelta (обмен идёт крючком),
//! валюты — монета и встречный токен. Ссылка на пул — адрес крючка (там состояние, события и квотер), ключ — для обмена.
//! Сам крючок (pool_manager(), N_COINS, coins) сверяется при каждом чтении состояния (load_inf_stable.rs).
//! Кто вызывает: pools/verify.rs (ветка Proto::InfinityStable), pools/discover_v4.rs (история и новые пулы Initialize).
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_check.rs, abi_inf_stable.rs, key.rs, discover.rs.
//! Настройки: dex.pool_manager, dex.stable_hooks [DEX].
//!
//! Источники: CLPoolManager.poolIdToPoolKey — infinity-core 3c5ef1be src/pool-cl/CLPoolManager.sol#L41, #L117 (ключ
//! пишется при initialize); проверка флагов крючка при initialize — src/libraries/Hooks.sol#L47-L60 (битмап parameters
//! = getHooksRegistrationBitmap() крючка); крючок привязан к одному пулу — beforeInitialize L1646-L1657 (is_binded)
//! verified-кода CLStableSwapHook (abi_inf_stable.rs).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_check::IInfinityKeys;
use super::abi_inf_stable::SWAP_BITS;
use super::discover::{Found, sorted};
use super::families::Family;
use super::key::{InfinityPoolKeyData, PoolKey, infinity_hook_bits};
use super::verify::{Candidate, Verdict};

/// Проверка пачки кандидатов частями по `size` вызовов. Возвращает: итог по каждому (сбой узла — Failed). Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<(B256, Address), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = (|| {
            let id = B256::from_str(&c.pool).map_err(|_| {
                format!(
                    "«{}» — не PoolId (пул Infinity Stable ищется по PoolId ключа)",
                    c.pool
                )
            })?;
            let manager = ctx
                .dex_addr("dex.pool_manager", c.family.id)
                .map_err(|e| e.message)?;
            Ok((id, manager))
        })();
        if let Ok((id, manager)) = &plan {
            let data = IInfinityKeys::poolIdToPoolKeyCall { id: *id }.abi_encode();
            calls.push((*manager, Bytes::from(data)));
        }
        plans.push(plan);
    }
    let mut got = call::try_chunks(ctx, &calls, "latest", size)
        .await
        .into_iter();
    let mut out = Vec::with_capacity(cands.len());
    for (c, plan) in cands.iter().zip(plans) {
        let (id, manager) = match plan {
            Ok(p) => p,
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        out.push(match got.next().unwrap_or(Err("нет ответа".into())) {
            Err(node) => Verdict::Failed(node),
            Ok(answer) => match decide(ctx, c, id, manager, answer) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Решение по ответу poolIdToPoolKey (без сети, кроме чтения настроек). Возвращает: найденный пул или причину.
fn decide(
    ctx: &Ctx,
    c: &Candidate,
    id: B256,
    manager: Address,
    answer: Result<Bytes, String>,
) -> Result<Found, String> {
    let raw = answer.map_err(|w| format!("ключ пула не прочитан: {w}"))?;
    let k = IInfinityKeys::poolIdToPoolKeyCall::abi_decode_returns(&raw)
        .map_err(|e| format!("ответ не по ABI: {e}"))?;
    if k.poolManager != manager {
        return Err(format!(
            "менеджер не знает этот PoolId (в ключе менеджер {}, у DEX {} — {manager})",
            k.poolManager, c.family.id
        ));
    }
    let key = InfinityPoolKeyData {
        currency0: k.currency0,
        currency1: k.currency1,
        hooks: k.hooks,
        manager: k.poolManager,
        fee: k.fee.to(),
        parameters: k.parameters,
    };
    if PoolKey::Infinity(key.clone()).id() != id {
        return Err(format!("ключ пула не даёт PoolId {id}"));
    }
    let found = found(ctx, c.family, key)?;
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

/// Найденный пул по ключу Infinity CL (из poolIdToPoolKey или события Initialize). Принимает: вид, ключ.
/// Возвращает: Found (ссылка — адрес крючка, coin = currency0 до выбора встречного токена) или причину: крючок не в
/// dex.stable_hooks, нет флагов обмена крючком, список не задан. Ошибок нет.
pub fn found(ctx: &Ctx, f: &'static Family, key: InfinityPoolKeyData) -> Result<Found, String> {
    if !listed(ctx, f, key.hooks)? {
        return Err(format!(
            "крючок {:#x} не в dex.stable_hooks DEX {} (адреса — из официального списка PancakeSwap)",
            key.hooks, f.id
        ));
    }
    let bits = infinity_hook_bits(key.parameters);
    if bits & SWAP_BITS != SWAP_BITS {
        return Err(format!(
            "в parameters ключа нет beforeSwap + beforeSwapReturnsDelta (флаги {bits:#06x}) — обмен шёл бы мимо крючка"
        ));
    }
    Ok(Found {
        family: f,
        reference: PoolRef::Contract(key.hooks),
        token0: key.currency0,
        token1: key.currency1,
        coin: key.currency0,
        // Комиссия CL-пула и tickSpacing обмену не нужны: крючок забирает весь вход (beforeSwap → amountToSwap = 0,
        // CLPool.swap выходит сразу — infinity-core CLPool.sol#L251-L252); комиссия — у крючка (StableData.fee).
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        // Крючок — это сам пул (вся его математика — math/stableswap_ng.rs): группа крючка и проба его влияния к нему
        // не относятся (решение 32 — для крючков поверх CL-математики).
        hooks: None,
        key: Some(PoolKey::Infinity(key)),
    })
}

/// Крючок в dex.stable_hooks DEX. Ошибки (текст): список не задан или не читается.
pub fn listed(ctx: &Ctx, f: &Family, hook: Address) -> Result<bool, String> {
    let list = ctx
        .dex_list("dex.stable_hooks", f.id)
        .map_err(|e| e.message)?;
    Ok(list
        .iter()
        .filter_map(|s| Address::from_str(s.trim()).ok())
        .any(|a| a == hook))
}
