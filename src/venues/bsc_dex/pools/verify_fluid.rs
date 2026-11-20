//! Файл: venues/bsc_dex/pools/verify_fluid.rs
//! Что делает: решение 440 — проверка пула Fluid DEX T1: фабрика dex.factory isDex(pool) = да (адрес пула — тот, что
//! фабрика вычисляет по DEX_ID() самого пула: factory/main.sol L253-L261 — поддельный контракт так не пройдёт),
//! constantsView() пула — фабрика = dex.factory, токены пула (метка нативной монеты 0xEeee… → dex.native_wrapped) =
//! монета и встречный токен в любом порядке. Пара записи — по возрастанию адреса (как у всех видов); порядок самого
//! пула нужен обмену (swap0to1) — его читает load_fluid.rs.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_fluid.rs, discover.rs (Found, sorted), verify.rs, stable_native.rs.
//! Настройки: dex.factory [DEX], dex.native_wrapped [bsc_dex].
//!
//! Источник: Instadapp/fluid-contracts-public@404d73f — contracts/protocols/dex/factory/main.sol L246-L261,
//! poolT1/coreModule/core/main.sol L964-L984 (constantsView), coreModule/structs.sol L130-L145.

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_fluid::{IFluidDexFactory as F, IFluidDexT1 as P, NATIVE};
use super::discover::{Found, sorted};
use super::stable_native;
use super::verify::{Candidate, Verdict};

/// Вызовов на кандидата: isDex (фабрика), constantsView (пул).
const PER: usize = 2;

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (нет dex.factory, не адрес — отказ с причиной;
/// сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let wrapped = stable_native::wrapped(ctx);
    let mut plans: Vec<Result<(Address, Address), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = Address::from_str(&c.pool)
            .map_err(|_| format!("«{}» — не адрес контракта пула Fluid", c.pool))
            .and_then(|pool| {
                ctx.dex_addr("dex.factory", c.family.id)
                    .map(|f| (pool, f))
                    .map_err(|e| e.message)
            });
        if let Ok((pool, fa)) = &plan {
            calls.push((*fa, F::isDexCall { dex_: *pool }.abi_encode().into()));
            calls.push((*pool, P::constantsViewCall {}.abi_encode().into()));
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
            Ok(a) => match decide(c, pool, factory, wrapped, &a) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Токен пула → токен пары (метка нативной монеты → обёртка). Ошибка (текст) — метка есть, обёртки нет.
fn coin(t: Address, wrapped: Option<Address>) -> Result<Address, String> {
    match (t == NATIVE, wrapped) {
        (false, _) => Ok(t),
        (true, Some(w)) => Ok(w),
        (true, None) => Err("нативная монета пула 0xEeee…, а dex.native_wrapped не задана".into()),
    }
}

/// Решение по ответам (без сети): isDex(pool) фабрики и constantsView() пула. Принимает: кандидата, пул, фабрику DEX,
/// обёртку нативной монеты, ответы по порядку (откат — Err с текстом). Возвращает: найденный пул (пара по
/// возрастанию) или причину.
pub fn decide(
    c: &Candidate,
    pool: Address,
    factory: Address,
    wrapped: Option<Address>,
    answers: &[Result<Bytes, String>],
) -> Result<Found, String> {
    let at = |i: usize| match answers.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("фабрика или пул Fluid откатили вызов: {r}")),
        None => Err("фабрика Fluid: нет ответа".to_string()),
    };
    let listed = F::isDexCall::abi_decode_returns(at(0)?)
        .map_err(|e| format!("isDex({pool:#x}) не по ABI: {e}"))?;
    if !listed {
        return Err(format!(
            "пул {pool:#x} не из реестра фабрики Fluid (isDex = нет) — dex.factory DEX {}",
            c.family.id
        ));
    }
    let cv = P::constantsViewCall::abi_decode_returns(at(1)?)
        .map_err(|e| format!("constantsView() пула {pool:#x} не по ABI: {e}"))?;
    if cv.factory != factory {
        return Err(format!(
            "фабрика пула {pool:#x} = {:#x}, у DEX {} — {factory:#x} (dex.factory)",
            cv.factory, c.family.id
        ));
    }
    let (t0, t1) = (coin(cv.token0, wrapped)?, coin(cv.token1, wrapped)?);
    let want = sorted(c.coin, c.quote);
    if t0 == t1 || sorted(t0, t1) != want {
        return Err(format!(
            "токены пула Fluid {pool:#x} {:#x} / {:#x} ≠ монета {} и встречный {}",
            cv.token0, cv.token1, c.coin, c.quote
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
