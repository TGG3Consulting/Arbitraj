//! Файл: venues/bsc_dex/pools/verify_curve_ng.rs
//! Что делает: проверка пула Curve StableSwap-NG (решение 275) из справочника на своём узле. Кто подтверждает пул
//! (решение 59) — реестр фабрики NG dex.factory: `get_coins(pool)` возвращает монеты пула, только если пул развёрнут
//! этой фабрикой (pool_data пуст у чужого адреса — пустой список), и `is_meta(pool)` = false (метапулы — другая
//! реализация CurveStableSwapMetaNG, этим видом не берутся). Пул принимается, если монета и встречный токен — среди
//! монет пула из реестра (CLAUDE.md: адрес токена у биржи = адрес в пуле). Вызовы — частями по dex.verify_batch.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_curve_ng.rs, discover.rs, verify.rs.
//! Настройки: dex.factory [DEX].
//!
//! Источник: curvefi/stableswap-ng 3332cd4, contracts/main/CurveStableSwapFactoryNG.vy — get_coins L175-L183
//! (`self.pool_data[_pool].coins`), is_meta L430-L437.

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_curve_ng::ICurveNgFactory as F;
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Вызовов на кандидата: get_coins, is_meta.
const PER: usize = 2;

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (нет dex.factory или пул не из реестра —
/// отказ с причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Address, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = plan(ctx, c);
        if let Ok((pool, fa)) = plan {
            calls.push((fa, F::get_coinsCall { pool }.abi_encode().into()));
            calls.push((fa, F::is_metaCall { pool }.abi_encode().into()));
        }
        plans.push(plan.map(|(pool, _)| pool));
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let pool = match plan {
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
            Ok(a) => match decide(c, pool, &a) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Адрес пула кандидата и фабрика DEX. Возвращает: (пул, фабрика) или причину отказа. Ошибок нет.
fn plan(ctx: &Ctx, c: &Candidate) -> Result<(Address, Address), String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула Curve NG", c.pool))?;
    let factory = ctx
        .dex_addr("dex.factory", c.family.id)
        .map_err(|e| e.message)?;
    Ok((pool, factory))
}

/// Решение по ответам реестра (без сети): get_coins(pool) и is_meta(pool). Принимает: кандидата, пул, ответы (откат
/// вызова — Err с текстом). Возвращает: найденный пул или причину отказа.
pub fn decide(
    c: &Candidate,
    pool: Address,
    answers: &[Result<Bytes, String>],
) -> Result<Found, String> {
    let at = |i: usize| match answers.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("реестр фабрики Curve NG откатил вызов: {r}")),
        None => Err("реестр фабрики Curve NG: нет ответа".to_string()),
    };
    let coins = F::get_coinsCall::abi_decode_returns(at(0)?)
        .map_err(|e| format!("get_coins({pool:#x}) не по ABI: {e}"))?;
    if coins.len() < 2 {
        return Err(format!(
            "пул {pool:#x} не из реестра фабрики Curve NG (get_coins пуст) — dex.factory DEX {}",
            c.family.id
        ));
    }
    let meta = F::is_metaCall::abi_decode_returns(at(1)?)
        .map_err(|e| format!("is_meta({pool:#x}) не по ABI: {e}"))?;
    if meta {
        return Err(format!(
            "пул {pool:#x} — метапул Curve NG (другая реализация), видом curve_stableswap_ng не берётся"
        ));
    }
    if !coins.contains(&c.coin) || !coins.contains(&c.quote) {
        return Err(format!(
            "среди монет пула Curve NG {pool:#x} нет пары {} / {}",
            c.coin, c.quote
        ));
    }
    let (token0, token1) = sorted(c.coin, c.quote);
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0,
        token1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    })
}
