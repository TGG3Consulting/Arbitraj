//! Файл: venues/bsc_dex/pools/verify_metric.rs
//! Что делает: решение 285 — проверка пула Metric OMM: реестр фабрики dex.factory isPool(pool) = да, getImmutables()
//! пула — фабрика = dex.factory, токены пула = монета и встречный токен (в любом порядке: пул хранит token0/token1 не
//! по возрастанию адреса — факт цепи, WBNB/USDT 0xf3eb…afd6: token0 = WBNB). Пара записи — по возрастанию (как у всех
//! видов); порядок самого пула нужен обмену (zeroForOne) — его читает load_metric.rs.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_metric.rs, discover.rs (Found, sorted), verify.rs.
//! Настройки: dex.factory [DEX].

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_metric::{IMetricOmmFactory as F, IMetricOmmPool as P};
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Вызовов на кандидата: isPool (фабрика), getImmutables (пул).
const PER: usize = 2;

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (нет dex.factory, не адрес — отказ с причиной;
/// сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<(Address, Address), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = Address::from_str(&c.pool)
            .map_err(|_| format!("«{}» — не адрес контракта пула Metric", c.pool))
            .and_then(|pool| {
                ctx.dex_addr("dex.factory", c.family.id)
                    .map(|f| (pool, f))
                    .map_err(|e| e.message)
            });
        if let Ok((pool, fa)) = &plan {
            calls.push((*fa, F::isPoolCall { pool: *pool }.abi_encode().into()));
            calls.push((*pool, P::getImmutablesCall {}.abi_encode().into()));
        }
        plans.push(plan);
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let (pool, factory) = match plan {
            Ok(p) => p,
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let part: Vec<call::Piece> = got.by_ref().take(PER).collect();
        let answers: Result<Vec<_>, String> = part.into_iter().collect();
        out.push(match answers {
            Err(node) => Verdict::Failed(node),
            Ok(a) => match decide(c, pool, factory, &a) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Решение по ответам (без сети): isPool(pool) фабрики и getImmutables() пула. Принимает: кандидата, пул, фабрику
/// DEX, ответы по порядку (откат — Err с текстом). Возвращает: найденный пул (пара по возрастанию) или причину.
pub fn decide(
    c: &Candidate,
    pool: Address,
    factory: Address,
    answers: &[Result<Bytes, String>],
) -> Result<Found, String> {
    let at = |i: usize| match answers.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("фабрика или пул Metric откатили вызов: {r}")),
        None => Err("фабрика Metric: нет ответа".to_string()),
    };
    let listed = F::isPoolCall::abi_decode_returns(at(0)?)
        .map_err(|e| format!("isPool({pool:#x}) не по ABI: {e}"))?;
    if !listed {
        return Err(format!(
            "пул {pool:#x} не из реестра фабрики Metric (isPool = нет) — dex.factory DEX {}",
            c.family.id
        ));
    }
    let im = P::getImmutablesCall::abi_decode_returns(at(1)?)
        .map_err(|e| format!("getImmutables() пула {pool:#x} не по ABI: {e}"))?;
    if im.factory != factory {
        return Err(format!(
            "фабрика пула {pool:#x} = {:#x}, у DEX {} — {factory:#x} (dex.factory)",
            im.factory, c.family.id
        ));
    }
    let want = sorted(c.coin, c.quote);
    if sorted(im.token0, im.token1) != want {
        return Err(format!(
            "токены пула Metric {pool:#x} {:#x} / {:#x} ≠ монета {} и встречный {}",
            im.token0, im.token1, c.coin, c.quote
        ));
    }
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0: want.0,
        token1: want.1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    })
}
