//! Файл: venues/bsc_dex/pools/load_balancer_fixed.rs
//! Что делает: решение 289 — неизменяемое у пула Balancer V2, читается один раз на пул и держится в памяти до
//! перезапуска: id пула (getPoolId), вид кривой — какая фабрика пул создала (isPoolFromFactory у dex.factory — весовой,
//! у dex.balancer_stable_factory — стейблсвап), множители точности 10^(18 − decimals) токенов пула (getPoolTokens
//! хранилища + decimals токенов). Вынесено из load_balancer.rs (предел размера файла).
//! Кто вызывает: pools/load_balancer.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer.rs.
//! Настройки: dex.factory, dex.balancer_stable_factory [DEX].
//!
//! Источник: BasePool.sol (весовой) L474-L481 / (стейбл) L510-L517 — множитель от decimals, decimals > 18 пул не
//! принимает (Math.sub(18, decimals)); BasePoolFactory.sol L48-L60 (isPoolFromFactory); BasePool.sol L159-L161 (getPoolId).

use std::collections::HashMap;
use std::sync::Mutex;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer::{
    IBalancerFactory as F, IBalancerPool as P, IBalancerToken as T, IBalancerVault as V,
};
use super::families::Family;

/// Неизменяемое у пула: id, весовой ли (иначе стейблсвап), множители точности по токену.
#[derive(Clone)]
pub struct Fixed {
    pub id: B256,
    pub weighted: bool,
    pub scaling: HashMap<Address, U256>,
}

/// Память неизменяемого по (площадка DEX, адрес пула) — решение 435 (433-и): пул с тем же адресом в другой сети — другой.
static FIXED: Mutex<Option<HashMap<(&'static str, Address), Fixed>>> = Mutex::new(None);

/// Неизменяемое пула: из памяти или с узла (два прохода: id и фабрики, затем токены и их decimals). Ошибки: узел, ответ
/// не по ABI, пул не из фабрик DEX, decimals > 18.
pub async fn fixed(
    ctx: &Ctx,
    f: &Family,
    vault: Address,
    pool: Address,
    tag: &str,
) -> GatewayResult<Fixed> {
    if let Some(x) = FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(ctx.place.id, pool)).cloned())
    {
        return Ok(x);
    }
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Balancer {pool}: {w}"));
    let wf = ctx.dex_addr("dex.factory", f.id)?;
    let sf = ctx.dex_addr("dex.balancer_stable_factory", f.id).ok();
    let mut calls: Vec<(Address, Bytes)> = vec![
        (pool, P::getPoolIdCall {}.abi_encode().into()),
        (wf, F::isPoolFromFactoryCall { pool }.abi_encode().into()),
    ];
    if let Some(s) = sf {
        calls.push((s, F::isPoolFromFactoryCall { pool }.abi_encode().into()));
    }
    let got = call::try_many(ctx, &calls, tag).await?;
    let ok = |i: usize| {
        got.get(i)
            .and_then(|r| r.as_ref().ok())
            .ok_or_else(|| bad(format!("вызов {i} откатился")))
    };
    let id = call::decode::<P::getPoolIdCall>(ok(0)?)?;
    let weighted = call::decode::<F::isPoolFromFactoryCall>(ok(1)?)?;
    let stable = sf.is_some() && call::decode::<F::isPoolFromFactoryCall>(ok(2)?)?;
    if !weighted && !stable {
        return Err(bad(
            "пул не из фабрик DEX (dex.factory / dex.balancer_stable_factory)".into(),
        ));
    }
    let tokens = call::view(ctx, vault, &V::getPoolTokensCall { poolId: id }, tag)
        .await?
        .tokens;
    let calls: Vec<(Address, Bytes)> = tokens
        .iter()
        .map(|t| (*t, T::decimalsCall {}.abi_encode().into()))
        .collect();
    let got = call::try_many(ctx, &calls, tag).await?;
    let mut scaling = HashMap::new();
    for (t, r) in tokens.iter().zip(got) {
        let d =
            call::decode::<T::decimalsCall>(&r.map_err(|w| bad(format!("decimals {t}: {w}")))?)?;
        if d > 18 {
            return Err(bad(format!(
                "decimals {t} = {d} > 18 (пул такой токен не принимает)"
            )));
        }
        scaling.insert(*t, U256::from(10u8).pow(U256::from(18 - d)));
    }
    let x = Fixed {
        id,
        weighted,
        scaling,
    };
    FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert((ctx.place.id, pool), x.clone());
    Ok(x)
}

/// Неизменяемое пула из памяти без обращения к узлу (пачка каждого блока, per_block_balancer.rs). None — пул ещё не
/// читался. Принимает: площадку DEX, пул. Ошибок нет.
pub fn cached(venue: &'static str, pool: Address) -> Option<Fixed> {
    FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(venue, pool)).cloned())
}
