//! Файл: venues/bsc_dex/pools/load_balancer_more_fixed.rs
//! Что делает: решение 473 (T450-03…07) — неизменяемое у пула Balancer V2 вида dex.balancer_pool, читается один раз на
//! пул вместе с load_balancer_fixed.rs и держится в памяти до перезапуска: токены по порядку хранилища, множители
//! FixedPoint 1e18·10^(18 − decimals), индекс BPT пула (composable stable, stable phantom, linear — getBptIndex;
//! managed — позиция адреса пула среди токенов), поставщики курсов (composable stable, meta-stable — getRateProviders),
//! индексы главного и обёрнутого токена (linear — getMainIndex / getWrappedIndex).
//! Кто вызывает: pools/load_balancer_fixed.rs (fixed), load_balancer_more.rs (calls / decode).
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer_more.rs, balancer_pool_type.rs.
//! Настройки: dex.balancer_pool [DEX]; dex.multicall3 [bsc_dex] (время блока для кеша курсов — поле time).
//!
//! Источник: ComposableStablePoolStorage.sol (20240223-v6) L184 getBptIndex, L289-L298 getRateProviders (по одному на
//! токен, у BPT — нулевой адрес); MetaStablePool.sol (20210727) L448-L453; StablePhantomPool.sol (20211208) L172;
//! LinearPool.sol (20211208) L158-L168; ManagedPool.sol (20221021) L713-L718 (_getPoolTokens — ComposablePoolLib.dropBpt:
//! BPT первым среди токенов); BasePool._computeScalingFactor — FixedPoint.ONE · 10^(18 − decimals) (20240223-v6
//! BasePool.sol L638-L646).

use std::collections::HashMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::balancer_math::one;
use super::super::node::call;
use super::abi_balancer_more::{IComposable, ILinear, IMetaStable};
use super::balancer_pool_type::PoolType;

/// Неизменяемое пула вида dex.balancer_pool (и адрес Multicall3 — из настройки при каждом чтении).
#[derive(Clone, Debug)]
pub struct More {
    pub ty: PoolType,
    pub tokens: Vec<Address>,
    /// Множитель FixedPoint от decimals: 1e18·10^(18 − decimals), по порядку tokens.
    pub dec: Vec<U256>,
    pub bpt: Option<usize>,
    /// Поставщик курса по порядку tokens (нулевой адрес — нет); пусто у видов без поставщиков.
    pub providers: Vec<Address>,
    pub main: usize,
    pub wrapped: usize,
    /// Multicall3 (dex.multicall3) — время блока в пачке чтения (кеш курсов); читается при каждом чтении пула.
    pub time: Option<Address>,
}

/// Неизменяемое вида. Принимает: окружение, вид, пул, токены хранилища, множители 10^(18 − decimals) по токену, блок.
/// Ошибки: узел, ответ не по ABI, индекс BPT / главного / обёрнутого не тот, поставщиков не по числу токенов.
pub async fn read(
    ctx: &Ctx,
    ty: PoolType,
    pool: Address,
    tokens: &[Address],
    scaling: &HashMap<Address, U256>,
    tag: &str,
) -> GatewayResult<More> {
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Balancer {pool}: {w}"));
    let mut dec = Vec::with_capacity(tokens.len());
    for t in tokens {
        let s = scaling.get(t).ok_or_else(|| bad(format!("нет decimals токена {t}")))?;
        dec.push(one().checked_mul(*s).ok_or_else(|| bad("множитель не помещается".into()))?);
    }
    let bpt_call: Bytes = IComposable::getBptIndexCall {}.abi_encode().into();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    match ty {
        PoolType::Composable => {
            calls.push((pool, bpt_call));
            let p = IComposable::getRateProvidersCall {}.abi_encode();
            calls.push((pool, p.into()));
        }
        PoolType::Phantom => calls.push((pool, bpt_call)),
        PoolType::MetaStable => {
            let p = IMetaStable::getRateProvidersCall {}.abi_encode();
            calls.push((pool, p.into()));
        }
        PoolType::LinearV1 | PoolType::Linear => {
            calls.push((pool, ILinear::getMainIndexCall {}.abi_encode().into()));
            calls.push((pool, ILinear::getWrappedIndexCall {}.abi_encode().into()));
            calls.push((pool, bpt_call));
        }
        PoolType::Lbp | PoolType::Investment | PoolType::Managed => {}
    }
    let got = call::try_many(ctx, &calls, tag).await?;
    let at = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(bad(format!("вызов {i} неизменяемого откатился: {r}"))),
        None => Err(bad(format!("нет ответа на вызов {i} неизменяемого"))),
    };
    let idx = |v: U256| -> GatewayResult<usize> {
        usize::try_from(v)
            .ok()
            .filter(|x| *x < tokens.len())
            .ok_or_else(|| bad(format!("индекс {v} вне токенов пула")))
    };
    let mut m = More {
        ty,
        tokens: tokens.to_vec(),
        dec,
        bpt: None,
        providers: Vec::new(),
        main: 0,
        wrapped: 0,
        time: None,
    };
    match ty {
        PoolType::Composable | PoolType::Phantom => {
            m.bpt = Some(idx(call::decode::<IComposable::getBptIndexCall>(at(0)?)?)?);
            if ty == PoolType::Composable {
                m.providers = call::decode::<IComposable::getRateProvidersCall>(at(1)?)?;
            }
        }
        PoolType::MetaStable => {
            m.providers = call::decode::<IMetaStable::getRateProvidersCall>(at(0)?)?;
        }
        PoolType::LinearV1 | PoolType::Linear => {
            m.main = idx(call::decode::<ILinear::getMainIndexCall>(at(0)?)?)?;
            m.wrapped = idx(call::decode::<ILinear::getWrappedIndexCall>(at(1)?)?)?;
            m.bpt = Some(idx(call::decode::<IComposable::getBptIndexCall>(at(2)?)?)?);
        }
        PoolType::Managed => m.bpt = tokens.iter().position(|t| *t == pool),
        PoolType::Lbp | PoolType::Investment => {}
    }
    check(&m, pool).map_err(bad)?;
    Ok(m)
}

/// Согласованность неизменяемого (без сети): BPT — адрес самого пула; у линейного три разных индекса; поставщиков —
/// по числу токенов. Ошибки (текст): что не сошлось.
pub fn check(m: &More, pool: Address) -> Result<(), String> {
    if let Some(b) = m.bpt
        && m.tokens.get(b) != Some(&pool)
    {
        return Err(format!("токен {b} пула — не его BPT"));
    }
    if m.ty.bpt_excluded() || m.ty.linear() {
        m.bpt.ok_or("BPT пула нет среди его токенов")?;
    }
    if m.ty.linear() {
        let b = m.bpt.unwrap_or(m.main);
        if m.tokens.len() != 3 || m.main == m.wrapped || m.main == b || m.wrapped == b {
            return Err("линейный пул: не три токена или индексы совпали".into());
        }
    }
    let rated = matches!(m.ty, PoolType::Composable | PoolType::MetaStable);
    if rated && m.providers.len() != m.tokens.len() {
        return Err(format!("поставщиков курса {} ≠ токенов {}", m.providers.len(), m.tokens.len()));
    }
    Ok(())
}
