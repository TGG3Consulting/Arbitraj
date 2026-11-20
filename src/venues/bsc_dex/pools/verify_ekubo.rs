//! Файл: venues/bsc_dex/pools/verify_ekubo.rs
//! Что делает: проверка пула Ekubo из справочника индексатора (решение 447; адрес пула у источника — PoolId) на своём
//! узле одним проходом вызовов: ключ пула у реестра PoolKeyIndex (poolKeyById) и состояние пула у ядра (Core.sload
//! слота PoolId). Пул берётся, только если: ключ зарегистрирован; keccak256 ключа = PoolId; пул создан (слово состояния
//! не ноль); токены ключа = монета и встречный токен (адрес монеты — от биржи); token0 — не нативный ETH (адрес 0: шаг
//! исполнителя и граф работают с ERC-20); расширение — нет или MEVCapture из dex.ekubo_mev_capture (другие расширения
//! меняют обмен — TWAMM исполняет ордера до обмена, их математики нет). Ключ запоминается (ekubo_keys.rs).
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_ekubo.rs, ekubo_keys.rs, math/ekubo_config.rs, discover.rs.
//! Настройки: dex.ekubo_core, dex.ekubo_key_index, dex.ekubo_mev_capture [DEX].
//!
//! Источник — EkuboProtocol/evm-contracts@02e0d82f src/PoolKeyIndex.sol L16 (poolKeyById), L61-L63 (не зарегистрирован —
//! token1 = 0); @61d09daa src/types/poolKey.sol L34-L39 (PoolId), src/types/poolState.sol L28-L32 (создан — слово ≠ 0),
//! src/math/constants.sol L270 (NATIVE_TOKEN_ADDRESS = 0), src/types/poolConfig.sol L80-L84 (расширение).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::math::ekubo_config::{self, Shape};
use super::super::node::call;
use super::abi_ekubo::{IEkuboKeyIndex, pool_id, sload_data, sload_words};
use super::discover::{Found, sorted};
use super::ekubo_keys;
use super::verify::{Candidate, Verdict};

/// Адреса DEX для проверки: ядро, реестр, MEVCapture (None — не задан).
struct Addrs {
    core: Address,
    index: Address,
    mev: Option<Address>,
}

/// Проверка пачки кандидатов Ekubo частями по `size` вызовов. Возвращает: итог по каждому. Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<(B256, Addrs), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = (|| {
            let id = B256::from_str(&c.pool).map_err(|_| format!("«{}» — не PoolId", c.pool))?;
            let core = ctx
                .dex_addr("dex.ekubo_core", c.family.id)
                .map_err(|e| e.message)?;
            let index = ctx
                .dex_addr("dex.ekubo_key_index", c.family.id)
                .map_err(|e| e.message)?;
            let mev = ctx.dex_addr("dex.ekubo_mev_capture", c.family.id).ok();
            Ok((id, Addrs { core, index, mev }))
        })();
        if let Ok((id, a)) = &plan {
            let key = IEkuboKeyIndex::poolKeyByIdCall { poolId: *id }.abi_encode();
            calls.push((a.index, Bytes::from(key)));
            calls.push((a.core, sload_data(&[*id])));
        }
        plans.push(plan);
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let none = || Err("нет ответа".to_string());
    let mut out = Vec::with_capacity(cands.len());
    for (c, plan) in cands.iter().zip(plans) {
        let (id, a) = match plan {
            Ok(p) => p,
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let key = got.next().unwrap_or_else(none);
        let state = got.next().unwrap_or_else(none);
        out.push(match (key, state) {
            (Err(node), _) | (_, Err(node)) => Verdict::Failed(node),
            (Ok(key), Ok(state)) => match decide(c, id, &a, key, state) {
                Ok(f) => {
                    if let Some(cfg) = f.1 {
                        ekubo_keys::remember(ctx.place.id, id, cfg);
                    }
                    Verdict::Taken(Box::new(f.0))
                }
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Решение по ответам (без сети). Принимает: кандидата, PoolId, адреса, ответ poolKeyById, ответ sload.
/// Возвращает: найденный пул и config ключа или причину отказа.
fn decide(
    c: &Candidate,
    id: B256,
    a: &Addrs,
    key: Result<Bytes, String>,
    state: Result<Bytes, String>,
) -> Result<(Found, Option<B256>), String> {
    let key = key.map_err(|w| format!("PoolKeyIndex.poolKeyById откатился: {w}"))?;
    let k = IEkuboKeyIndex::poolKeyByIdCall::abi_decode_returns(&key)
        .map_err(|e| format!("ответ poolKeyById не по ABI: {e}"))?;
    if k.token1 == Address::ZERO {
        return Err("пула нет в реестре ключей PoolKeyIndex (dex.ekubo_key_index) — ключ не известен".into());
    }
    if pool_id(k.token0, k.token1, k.config) != id {
        return Err(format!("ключ пула из PoolKeyIndex даёт другой PoolId, а не {id}"));
    }
    let state = state.map_err(|w| format!("Core.sload откатился: {w}"))?;
    let words = sload_words(&state, 1).ok_or("ответ Core.sload не 32 байта")?;
    if words[0].is_zero() {
        return Err("пул не создан в ядре (состояние — ноль)".into());
    }
    if k.token0 == Address::ZERO {
        return Err("token0 пула — нативный ETH (адрес 0): шаг исполнителя и граф — только ERC-20".into());
    }
    let ext = ekubo_config::extension(&k.config);
    if ext != Address::ZERO && Some(ext) != a.mev {
        return Err(format!(
            "расширение пула {ext} не поддержано (только без расширения или MEVCapture из dex.ekubo_mev_capture)"
        ));
    }
    if (k.token0, k.token1) != sorted(c.coin, c.quote) {
        return Err(format!(
            "токены пула {} / {} ≠ монета {} и встречный {} (адрес монеты — от биржи)",
            k.token0, k.token1, c.coin, c.quote
        ));
    }
    let spacing = match ekubo_config::shape(&k.config) {
        Shape::Concentrated { spacing } => Some(spacing as i32),
        Shape::FullRange | Shape::Stable { .. } => None,
    };
    let found = Found {
        family: c.family,
        reference: PoolRef::Singleton {
            manager: a.core,
            id,
        },
        token0: k.token0,
        token1: k.token1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: spacing,
        bin_step: None,
        key: None,
        hooks: None,
    };
    Ok((found, Some(k.config)))
}
