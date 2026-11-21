//! Файл: venues/bsc_dex/pools/verify_maverick.rs
//! Что делает: проверка пула Maverick V2 (решение 122 F) из справочника или события создания на своём узле. «Кто
//! подтверждает пул» (решение 59) — реестр фабрики DEX: isFactoryPool(pool) = true (поддельный контракт может
//! вернуть из factory() что угодно, реестр — нет). Плюс поля пула: tokenA/tokenB = монета и встречный токен
//! (CLAUDE.md: адрес токена у биржи = адрес в пуле) и permissionedSwap() = false (иначе обмен разрешён только
//! accessor-у пула — IMaverickV2Pool.sol L201-L206, IMaverickV2Factory.sol L237-L238). Один проход Multicall3 частями
//! по dex.verify_batch; откат вызова — ответ (пул отклонён с причиной), сбой узла — «не проверен».
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_maverick.rs, discover.rs, verify.rs.
//! Настройки: dex.factory [DEX].

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_maverick::{IMaverickV2Factory as F, IMaverickV2Pool as P};
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Вызовов на кандидата: isFactoryPool, tokenA, tokenB, permissionedSwap.
const PER: usize = 4;

/// Проверка пачки кандидатов Maverick V2. Возвращает: итог по каждому. Ошибок нет (адрес/фабрика не заданы — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Address, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = plan(ctx, c);
        if let Ok((pool, factory)) = &plan {
            calls.push((
                *factory,
                F::isFactoryPoolCall { pool: *pool }.abi_encode().into(),
            ));
            calls.push((*pool, P::tokenACall {}.abi_encode().into()));
            calls.push((*pool, P::tokenBCall {}.abi_encode().into()));
            calls.push((*pool, P::permissionedSwapCall {}.abi_encode().into()));
        }
        plans.push(plan.map(|(p, _)| p));
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

/// Адрес пула и фабрики кандидата. Возвращает: (пул, фабрика) или причину отказа. Ошибок нет.
fn plan(ctx: &Ctx, c: &Candidate) -> Result<(Address, Address), String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула Maverick V2", c.pool))?;
    let factory = ctx
        .dex_addr("dex.factory", c.family.id)
        .map_err(|e| e.message)?;
    Ok((pool, factory))
}

/// Решение по ответам (без сети): [isFactoryPool, tokenA, tokenB, permissionedSwap]. Возвращает: найденный пул или
/// причину отказа.
pub fn decide(c: &Candidate, pool: Address, a: &[Result<Bytes, String>]) -> Result<Found, String> {
    let get = |i: usize, name: &str| {
        a.get(i)
            .ok_or_else(|| format!("нет ответа {name}"))?
            .as_ref()
            .map_err(|r| format!("{name} пула {pool:#x} откатился: {r}"))
    };
    let not_abi = |name: &str, e: alloy::sol_types::Error| format!("ответ {name} не по ABI: {e}");
    let known = F::isFactoryPoolCall::abi_decode_returns(get(0, "isFactoryPool")?)
        .map_err(|e| not_abi("isFactoryPool", e))?;
    if !known {
        return Err(format!(
            "пул {pool:#x} не из фабрики DEX (isFactoryPool = false)"
        ));
    }
    let ta =
        P::tokenACall::abi_decode_returns(get(1, "tokenA")?).map_err(|e| not_abi("tokenA", e))?;
    let tb =
        P::tokenBCall::abi_decode_returns(get(2, "tokenB")?).map_err(|e| not_abi("tokenB", e))?;
    let (token0, token1) = sorted(c.coin, c.quote);
    if (ta, tb) != (token0, token1) {
        return Err(format!(
            "токены пула {ta}/{tb} не пара {} / {}",
            c.coin, c.quote
        ));
    }
    let closed = P::permissionedSwapCall::abi_decode_returns(get(3, "permissionedSwap")?)
        .map_err(|e| not_abi("permissionedSwap", e))?;
    if closed {
        return Err(format!(
            "пул {pool:#x}: обмен только для accessor пула (permissionedSwap = true)"
        ));
    }
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
