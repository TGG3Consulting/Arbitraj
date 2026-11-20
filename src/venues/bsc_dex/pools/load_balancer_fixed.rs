//! Файл: venues/bsc_dex/pools/load_balancer_fixed.rs
//! Что делает: решение 289 — неизменяемое у пула Balancer V2, читается один раз на пул и держится в памяти до
//! перезапуска: id пула (getPoolId), вид кривой — какая фабрика пул создала (isPoolFromFactory у dex.factory — весовой,
//! у dex.balancer_stable_factory — стейблсвап), множители точности 10^(18 − decimals) токенов пула (getPoolTokens
//! хранилища + decimals токенов). Вынесено из load_balancer.rs (предел размера файла).
//! Кто вызывает: pools/load_balancer.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_balancer.rs.
//! Решение 473: при заданной dex.balancer_pool — пулы только фабрики dex.factory (вид — из настройки), неизменяемое
//! вида читает load_balancer_more_fixed.rs (поле more).
//! Настройки: dex.factory, dex.balancer_stable_factory, dex.balancer_math (решение 450), dex.balancer_pool (решение
//! 473) [DEX]; dex.multicall3 [bsc_dex] (решение 473: время блока для кеша курсов).
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
use super::balancer_pool_type::{self, PoolType};
use super::families::Family;
use super::load_balancer_more_fixed::{self as more_fixed, More};

/// Неизменяемое у пула: id, весовой ли (иначе стейблсвап), множители точности по токену; v2 — версия библиотек пула
/// (решение 450, dex.balancer_math = v2; читается из настройки при каждом чтении — не из памяти).
#[derive(Clone)]
pub struct Fixed {
    pub id: B256,
    pub weighted: bool,
    pub scaling: HashMap<Address, U256>,
    pub v2: bool,
    /// Решение 473: вид dex.balancer_pool и его неизменяемое; None — весовой / стейблсвап (как раньше).
    pub more: Option<Box<More>>,
}

/// Решение 450: версия библиотек пулов ключа DEX — dex.balancer_math: v1 (не задана — v1, как раньше) или v2
/// (FixedPoint с точными степенями 1/2/4 и StableMath v2 — math/balancer_math.rs). Принимает: окружение, вид DEX.
/// Возвращает: true — v2. Ошибки (текст): значение не из списка; настройка не читается (ревью 450, п. 4: только «не
/// задана» — NotConfigured — значит v1, прочие ошибки чтения — ошибка, пул не читается).
pub fn math_v2(ctx: &Ctx, f: &Family) -> Result<bool, String> {
    match ctx.dex_text("dex.balancer_math", f.id) {
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(false),
        Err(e) => Err(format!("dex.balancer_math [{}]: {}", f.id, e.message)),
        Ok(v) => match v.trim() {
            "v1" => Ok(false),
            "v2" => Ok(true),
            other => Err(format!("dex.balancer_math [{}] = «{other}»: известны v1, v2", f.id)),
        },
    }
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
    let ty = balancer_pool_type::of(ctx, f).map_err(|w| ctx.not_set("dex.balancer_pool", w))?;
    // Решение 473: вид ключа — подписке на события (events_balancer.rs: доп. события только у ключа с видом).
    balancer_pool_type::remember(f, ty);
    if let Some(mut x) = FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(ctx.place.id, pool)).cloned())
        .filter(|x| kind(x) == ty)
    {
        live(ctx, f, &mut x).map_err(|w| ctx.not_set("dex.balancer_math", w))?;
        return Ok(x);
    }
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Balancer {pool}: {w}"));
    let wf = ctx.dex_addr("dex.factory", f.id)?;
    // Решение 473: вид задан — пулы только фабрики dex.factory, стейблсвап-фабрика не спрашивается.
    let sf = match ty {
        Some(_) => None,
        None => ctx.dex_addr("dex.balancer_stable_factory", f.id).ok(),
    };
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
    let more = match ty {
        Some(t) => Some(Box::new(more_fixed::read(ctx, t, pool, &tokens, &scaling, tag).await?)),
        None => None,
    };
    let mut x = Fixed {
        id,
        weighted: ty.map_or(weighted, PoolType::weighted),
        scaling,
        v2: false,
        more,
    };
    live(ctx, f, &mut x).map_err(|w| ctx.not_set("dex.balancer_math", w))?;
    FIXED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert((ctx.place.id, pool), x.clone());
    Ok(x)
}

/// Решение 473: вид неизменяемого (None — весовой / стейблсвап). Ошибок нет.
pub fn kind(x: &Fixed) -> Option<PoolType> {
    x.more.as_ref().map(|m| m.ty)
}

/// Что читается из настроек при каждом чтении пула (не из памяти): dex.balancer_math (решение 450; у вида
/// dex.balancer_pool не читается) или, у вида dex.balancer_pool, адрес Multicall3 для времени блока (решение 473). Принимает: окружение, вид DEX, неизменяемое.
/// Ошибки (текст): dex.balancer_math не читается или не из списка.
pub fn live(ctx: &Ctx, f: &Family, x: &mut Fixed) -> Result<(), String> {
    // Решение 473: у ключа с dex.balancer_pool версия библиотек задана видом — dex.balancer_math не читается.
    match x.more.as_mut() {
        Some(m) => m.time = super::super::node::multicall::address(ctx),
        None => x.v2 = math_v2(ctx, f)?,
    }
    Ok(())
}

/// Решение 473: текст отказа разбора, когда токены пула в хранилище не те, что в памяти (managed: TokenAdded /
/// TokenRemoved).
pub const TOKENS_CHANGED: &str = "токены пула в хранилище изменились — неизменяемое сброшено, перечитать";

/// Решение 473: сбросить неизменяемое пула в памяти (во всех площадках DEX) — следующее чтение возьмёт его с узла
/// (токены, decimals, BPT). Зовут: разбор пачки, у которого токены не совпали (pool — из id пула: старшие 20 байт,
/// PoolRegistry.sol L115-L138), и событие TokenAdded / TokenRemoved пула managed (events_balancer.rs). Ошибок нет.
pub fn forget(pool: Address) {
    if let Some(m) = FIXED.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.retain(|(_, a), _| *a != pool);
    }
}

/// Пул по id Balancer V2 — старшие 20 байт id (PoolRegistry.sol L115-L138). Ошибок нет.
pub fn pool_of(id: B256) -> Address {
    Address::from_slice(&id[..20])
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
