//! Файл: venues/bsc_dex/pools/load_balancer_v3_fixed.rs
//! Что делает: решение 441 (T433-F2) — неизменяемое у пула Balancer V3, читается один раз на пул и держится в памяти до
//! перезапуска: выпуск фабрики (balancer_v3_factories.rs, isPoolFromFactory), токены пула в порядке хранилища
//! (getPoolTokenInfo), веса (getNormalizedWeights — неизменяемые у весового пула) и нижние балансы (getMinTokenBalances —
//! у выпуска 20260115), наименьшая сумма обмена хранилища (getMinimumTradeAmount — неизменяемое хранилища) и проверка
//! крючков: пул с крючком, который участвует в обмене (shouldCallComputeDynamicSwapFee / BeforeSwap / AfterSwap), не
//! берётся — расчёт крючка не сверен с исходником.
//! Кто вызывает: pools/load_balancer_v3.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer_v3.rs, balancer_v3_factories.rs.
//! Настройки: dex.vault, dex.bv3_* [DEX вида balancer_v3].
//!
//! Источник: шапка abi_balancer_v3.rs; WeightedPool.sol (20241205) — веса immutable (_normalizedWeight0…7), у
//! 20260115 — и нижние балансы immutable (_minBalance0…7, WeightedPool.sol L70-L78); VaultStorage.sol L52
//! (_MINIMUM_TRADE_AMOUNT immutable); Vault.sol L225-L284 (крючки обмена — только по флагам пула).

use std::collections::HashMap;
use std::sync::Mutex;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer_v3::{IBalancerV3Factory as F, IBalancerV3Pool as P, IBalancerV3Vault as V};
use super::balancer_v3_factories::{self as fac, Release};
use super::families::Family;

/// Неизменяемое у пула: выпуск, токены по порядку хранилища, веса (весовой), нижние балансы (20260115), наименьшая сумма.
#[derive(Clone)]
pub struct Fixed {
    pub release: Release,
    pub tokens: Vec<Address>,
    pub weights: Vec<U256>,
    pub min_balances: Vec<U256>,
    pub min_trade: U256,
}

/// Память неизменяемого по (площадка DEX, адрес пула) — пул с тем же адресом в другой сети — другой (решение 435).
static FIXED: Mutex<Option<HashMap<(&'static str, Address), Fixed>>> = Mutex::new(None);

/// Неизменяемое пула: из памяти или с узла (два прохода: фабрики, крючки, токены и наименьшая сумма; затем веса /
/// нижние балансы). Ошибки: узел, ответ не по ABI, пул не из фабрик DEX, крючок обмена (BadResponse с причиной).
pub async fn fixed(
    ctx: &Ctx,
    f: &Family,
    vault: Address,
    pool: Address,
    tag: &str,
) -> GatewayResult<Fixed> {
    if let Some(x) = cached(ctx.place.id, pool) {
        return Ok(x);
    }
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Balancer V3 {pool}: {w}"));
    let list = fac::all(ctx, f.id).map_err(|w| ctx.err(ErrorCode::NotConfigured, w))?;
    let mut calls: Vec<(Address, Bytes)> = vec![
        (vault, V::getHooksConfigCall { pool }.abi_encode().into()),
        (vault, V::getPoolTokenInfoCall { pool }.abi_encode().into()),
        (vault, V::getMinimumTradeAmountCall {}.abi_encode().into()),
    ];
    for (a, _) in &list {
        calls.push((*a, F::isPoolFromFactoryCall { pool }.abi_encode().into()));
    }
    let got = call::try_many(ctx, &calls, tag).await?;
    let ok = |i: usize| {
        got.get(i)
            .and_then(|r| r.as_ref().ok())
            .ok_or_else(|| bad(format!("вызов {i} откатился")))
    };
    let hooks = call::decode::<V::getHooksConfigCall>(ok(0)?)?;
    if let Some(why) = swap_hook(&hooks) {
        return Err(bad(why));
    }
    let tokens = call::decode::<V::getPoolTokenInfoCall>(ok(1)?)?.tokens;
    let min_trade = call::decode::<V::getMinimumTradeAmountCall>(ok(2)?)?;
    let mut yes = Vec::with_capacity(list.len());
    for i in 0..list.len() {
        yes.push(call::decode::<F::isPoolFromFactoryCall>(ok(3 + i)?)?);
    }
    let release = fac::release(&list, &yes).map_err(bad)?;
    let weights = if release.weighted() {
        call::view(ctx, pool, &P::getNormalizedWeightsCall {}, tag).await?
    } else {
        Vec::new()
    };
    let min_balances = if release == Release::WeightedMin {
        call::view(ctx, pool, &P::getMinTokenBalancesCall {}, tag).await?
    } else {
        Vec::new()
    };
    let x = Fixed {
        release,
        tokens,
        weights,
        min_balances,
        min_trade,
    };
    FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert((ctx.place.id, pool), x.clone());
    Ok(x)
}

/// Крючок, участвующий в обмене (Vault.sol L225-L284: beforeSwap, динамическая комиссия, afterSwap). Принимает: настройки
/// крючков пула. Возвращает: причину отказа или None — крючков обмена нет. Ошибок нет.
pub fn swap_hook(h: &V::HooksConfig) -> Option<String> {
    let on = h.shouldCallComputeDynamicSwapFee || h.shouldCallBeforeSwap || h.shouldCallAfterSwap;
    let flags = format!(
        "динамическая комиссия {}, до обмена {}, после обмена {}",
        h.shouldCallComputeDynamicSwapFee, h.shouldCallBeforeSwap, h.shouldCallAfterSwap
    );
    let why = "расчёт крючка не сверен с исходником, пул не берётся (docs/TASKS.md, решение 441)";
    on.then(|| format!("у пула крючок обмена {:#x} ({flags}) — {why}", h.hooksContract))
}

/// Неизменяемое пула из памяти без обращения к узлу. None — пул ещё не читался. Принимает: площадку DEX, пул.
/// Ошибок нет.
pub fn cached(venue: &'static str, pool: Address) -> Option<Fixed> {
    FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(venue, pool)).cloned())
}
