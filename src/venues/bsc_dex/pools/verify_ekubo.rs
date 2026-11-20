//! Файл: venues/bsc_dex/pools/verify_ekubo.rs
//! Что делает: проверка пула Ekubo (решения 447, 454; адрес пула у источника — PoolId) на своём узле одним проходом
//! вызовов: ключ пула у реестра PoolKeyIndex (poolKeyById) — если ключа нет в памяти (ekubo_keys.rs: его кладёт история
//! и событие PoolInitialized ядра, history_ekubo.rs — ключ сверен с PoolId), и состояние пула у ядра (Core.sload слота
//! PoolId). Пул берётся, только если: ключ известен (реестр или история ядра); keccak256 ключа = PoolId; пул создан
//! (слово состояния не ноль); пара пула (нативный ETH — как обёртка dex.native_wrapped, решение 454) = монета и
//! встречный токен (адрес монеты — от биржи); расширение — нет или официальное из настроек (ekubo_ext.rs). Ключ
//! запоминается (ekubo_keys.rs). Правила ключа — ekubo_accept.rs.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_ekubo.rs, ekubo_keys.rs, ekubo_accept.rs, discover.rs.
//! Настройки: dex.ekubo_core, dex.ekubo_key_index [DEX]; через ekubo_accept.rs — dex.native_wrapped и адреса
//! расширений.
//!
//! Источник — EkuboProtocol/evm-contracts@02e0d82f src/PoolKeyIndex.sol L16 (poolKeyById), L61-L63 (не зарегистрирован —
//! token1 = 0); @61d09daa src/types/poolKey.sol L34-L39 (PoolId), src/types/poolState.sol L28-L32 (создан — слово ≠ 0).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_ekubo::{IEkuboKeyIndex, sload_data, sload_words};
use super::discover::{Found, sorted};
use super::ekubo_accept::{PoolKeyIn, accept};
use super::ekubo_keys::{self, Key};
use super::verify::{Candidate, Verdict};

/// План проверки кандидата: PoolId, ядро, реестр, ключ из памяти (None — спросить реестр).
struct Plan {
    id: B256,
    core: Address,
    index: Address,
    known: Option<Key>,
}

/// Проверка пачки кандидатов Ekubo частями по `size` вызовов. Возвращает: итог по каждому. Ошибок нет.
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Plan, String>> = Vec::new();
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
            let known = ekubo_keys::get(ctx.place.id, id);
            Ok(Plan {
                id,
                core,
                index,
                known,
            })
        })();
        if let Ok(p) = &plan {
            if p.known.is_none() {
                let key = IEkuboKeyIndex::poolKeyByIdCall { poolId: p.id }.abi_encode();
                calls.push((p.index, Bytes::from(key)));
            }
            calls.push((p.core, sload_data(&[p.id])));
        }
        plans.push(plan);
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let none = || Err("нет ответа".to_string());
    let mut out = Vec::with_capacity(cands.len());
    for (c, plan) in cands.iter().zip(plans) {
        let p = match plan {
            Ok(p) => p,
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let key = match p.known {
            Some(k) => Ok(Ok(k)),
            None => got.next().unwrap_or_else(none).map(Err),
        };
        let state = got.next().unwrap_or_else(none);
        out.push(match (key, state) {
            (Err(node), _) | (_, Err(node)) => Verdict::Failed(node),
            (Ok(key), Ok(state)) => match decide(ctx, c, &p, key, state) {
                Ok((f, k)) => {
                    ekubo_keys::remember(ctx.place.id, p.id, k);
                    Verdict::Taken(Box::new(f))
                }
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Ключ пула: из памяти (токены — по паре кандидата) или из ответа poolKeyById. Ошибки (текст): ответ не по ABI, пула
/// нет в реестре, нативный ключ без dex.native_wrapped.
fn key_of(
    c: &Candidate,
    key: Result<Key, Bytes>,
    wrapped: Option<Address>,
) -> Result<PoolKeyIn, String> {
    match key {
        Ok(k) => {
            let (a, b) = sorted(c.coin, c.quote);
            let (token0, token1) = if !k.native {
                (a, b)
            } else {
                // Нативный token0 ключа — обёртка в паре кандидата; другой токен пары — token1 ключа.
                let w = wrapped.ok_or("ключ пула с нативным ETH, а dex.native_wrapped не задана")?;
                (Address::ZERO, if a == w { b } else { a })
            };
            Ok(PoolKeyIn {
                token0,
                token1,
                config: k.config,
            })
        }
        Err(raw) => {
            let k = IEkuboKeyIndex::poolKeyByIdCall::abi_decode_returns(&raw)
                .map_err(|e| format!("ответ poolKeyById не по ABI: {e}"))?;
            if k.token1 == Address::ZERO {
                return Err("пула нет в реестре ключей PoolKeyIndex (dex.ekubo_key_index) и в истории ядра — ключ не известен".into());
            }
            Ok(PoolKeyIn {
                token0: k.token0,
                token1: k.token1,
                config: k.config,
            })
        }
    }
}

/// Решение по ответам (без сети). Принимает: окружение, кандидата, план, ключ (память или ответ реестра), ответ sload.
/// Возвращает: найденный пул и ключ для памяти или причину отказа.
fn decide(
    ctx: &Ctx,
    c: &Candidate,
    p: &Plan,
    key: Result<Key, Bytes>,
    state: Bytes,
) -> Result<(Found, Key), String> {
    let k = key_of(c, key, ctx.addr("dex.native_wrapped").ok())?;
    let words = sload_words(&state, 1).ok_or("ответ Core.sload не 32 байта")?;
    if words[0].is_zero() {
        return Err("пул не создан в ядре (состояние — ноль)".into());
    }
    let (mut found, key) = accept(ctx, c.family, p.core, p.id, k)?;
    if (found.token0, found.token1) != sorted(c.coin, c.quote) {
        return Err(format!(
            "токены пула {} / {} ≠ монета {} и встречный {} (адрес монеты — от биржи)",
            found.token0, found.token1, c.coin, c.quote
        ));
    }
    found.coin = c.coin;
    Ok((found, key))
}
