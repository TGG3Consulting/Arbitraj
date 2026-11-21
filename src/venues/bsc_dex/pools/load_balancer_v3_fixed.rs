//! Файл: venues/bsc_dex/pools/load_balancer_v3_fixed.rs
//! Что делает: решение 441 (T433-F2) — неизменяемое у пула Balancer V3, читается один раз на пул и держится в памяти до
//! перезапуска: выпуск фабрики (balancer_v3_factories.rs, isPoolFromFactory), токены пула в порядке хранилища
//! (getPoolTokenInfo), наименьшая сумма обмена хранилища (getMinimumTradeAmount — неизменяемое хранилища). Решение 452:
//! крючок комиссии (balancer_v3_hooks.rs: StableSurge, MEV Capture — берутся; beforeSwap / afterSwap и чужой крючок
//! динамической комиссии — пул не берётся) и неизменяемое выпуска вторым проходом (load_balancer_v3_extra.rs: веса,
//! нижние балансы, Gyro, LBP, QuantAMM).
//! Кто вызывает: pools/load_balancer_v3.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer_v3.rs, balancer_v3_factories.rs, balancer_v3_hooks.rs,
//! load_balancer_v3_extra.rs.
//! Настройки: dex.vault, dex.bv3_* (фабрики и крючки) [DEX вида balancer_v3].
//!
//! Источник: шапка abi_balancer_v3.rs; VaultStorage.sol L52 (_MINIMUM_TRADE_AMOUNT immutable); Vault.sol L225-L284
//! (крючки обмена — только по флагам пула); неизменяемость выпусков — шапка load_balancer_v3_extra.rs.

use std::collections::HashMap;
use std::sync::Mutex;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer_v3::{IBalancerV3Factory as F, IBalancerV3Vault as V};
use super::balancer_v3_factories::{self as fac, Release};
use super::balancer_v3_hooks::{self, HookKind};
use super::families::Family;
use super::load_balancer_v3_extra::{self as extra, Extra};

/// Неизменяемое у пула: выпуск, токены по порядку хранилища, наименьшая сумма, крючок комиссии (решение 452) и
/// неизменяемое выпуска (load_balancer_v3_extra.rs).
#[derive(Clone)]
pub struct Fixed {
    pub release: Release,
    pub tokens: Vec<Address>,
    pub min_trade: U256,
    pub hook: HookKind,
    pub extra: Extra,
    /// Решение 474: опорная сумма превью буферов ERC4626 (dex.bv3_buffer_preview_raw) — не неизменяемое пула: ставит
    /// load_balancer_v3::read у копии на каждое чтение; в памяти — None. None — буферы не читаются.
    pub buffer: Option<U256>,
}

/// Память неизменяемого по (площадка DEX, адрес пула) — пул с тем же адресом в другой сети — другой (решение 435).
static FIXED: Mutex<Option<HashMap<(&'static str, Address), Fixed>>> = Mutex::new(None);

/// Неизменяемое пула: из памяти или с узла (два прохода: фабрики, крючки, токены и наименьшая сумма; затем
/// неизменяемое выпуска). Ошибки: узел, ответ не по ABI, пул не из фабрик DEX, крючок не берётся (BadResponse с
/// причиной), не заданы списки фабрик или крючков (NotConfigured).
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
    let conf = |w: String| ctx.err(ErrorCode::NotConfigured, w);
    let list = fac::all(ctx, f.id).map_err(conf)?;
    let hooks = balancer_v3_hooks::lists(ctx, f.id).map_err(conf)?;
    let got = call::try_many(ctx, &first_calls(vault, pool, &list), tag).await?;
    let (release, tokens, min_trade, hook) = first(&list, &hooks, &got).map_err(bad)?;
    let more = call::try_many(ctx, &extra::calls(pool, release), tag).await?;
    let x = Fixed {
        release,
        tokens,
        min_trade,
        hook,
        extra: extra::decode(release, &more).map_err(bad)?,
        buffer: None,
    };
    FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert((ctx.place.id, pool), x.clone());
    Ok(x)
}

/// Вызовы первого прохода: getHooksConfig, getPoolTokenInfo, getMinimumTradeAmount хранилища, isPoolFromFactory каждой
/// фабрики (порядок fac::all). Ошибок нет.
pub fn first_calls(
    vault: Address,
    pool: Address,
    list: &[(Address, Release)],
) -> Vec<(Address, Bytes)> {
    let mut calls: Vec<(Address, Bytes)> = vec![
        (vault, V::getHooksConfigCall { pool }.abi_encode().into()),
        (vault, V::getPoolTokenInfoCall { pool }.abi_encode().into()),
        (vault, V::getMinimumTradeAmountCall {}.abi_encode().into()),
    ];
    for (a, _) in list {
        calls.push((*a, F::isPoolFromFactoryCall { pool }.abi_encode().into()));
    }
    calls
}

/// Разбор первого прохода (без сети). Принимает: фабрики, списки крючков, ответы по порядку first_calls. Возвращает:
/// (выпуск, токены, наименьшая сумма, крючок). Ошибки (текст): откат, не по ABI, пул не из фабрик, крючок не берётся.
pub fn first(
    list: &[(Address, Release)],
    hooks: &balancer_v3_hooks::Lists,
    got: &[Result<Bytes, String>],
) -> Result<(Release, Vec<Address>, U256, HookKind), String> {
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(w)) => Err(format!("вызов {i} откатился: {w}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let dec = |e: alloy::sol_types::Error| format!("ответ не по ABI: {e}");
    let cfg = V::getHooksConfigCall::abi_decode_returns(ok(0)?).map_err(dec)?;
    let info = V::getPoolTokenInfoCall::abi_decode_returns(ok(1)?).map_err(dec)?;
    let min_trade = V::getMinimumTradeAmountCall::abi_decode_returns(ok(2)?).map_err(dec)?;
    let mut yes = Vec::with_capacity(list.len());
    for i in 0..list.len() {
        yes.push(F::isPoolFromFactoryCall::abi_decode_returns(ok(3 + i)?).map_err(dec)?);
    }
    let release = fac::release(list, &yes)?;
    let hook = balancer_v3_hooks::kind(hooks, &cfg, release)?;
    Ok((release, info.tokens, min_trade, hook))
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
