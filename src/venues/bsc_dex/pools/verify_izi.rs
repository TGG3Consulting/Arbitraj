//! Файл: venues/bsc_dex/pools/verify_izi.rs
//! Что делает: решение 286 — проверка пула iZiSwap / Biswap V3: поля пула factory() = dex.factory, tokenX / tokenY =
//! монета и встречный токен (tokenX < tokenY — фабрика сортирует пару: iZiSwapFactory.sol L101-L104,
//! BiswapFactoryV3.sol L116-L119), fee(); затем реестр фабрики pool(tokenX, tokenY, fee) = сам пул (у iZiSwap fee —
//! uint24, у Biswap V3 — uint16: селектор по виду, abi_izi.rs). Два прохода: поля пулов, затем реестр по их fee.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_izi.rs, discover.rs (Found, sorted), verify.rs, families.rs.
//! Настройки: dex.factory [DEX].

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_izi::{IBiswapFactoryV3 as BF, IIziFactory as IF, IIziPool as P};
use super::discover::{Found, sorted};
use super::families::Proto;
use super::verify::{Candidate, Verdict};

/// Вызовов на кандидата в первом проходе: factory, tokenX, tokenY, fee.
const PER: usize = 4;

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (нет dex.factory, не адрес — отказ с причиной;
/// сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<(Address, Address), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = Address::from_str(&c.pool)
            .map_err(|_| format!("«{}» — не адрес контракта пула iZiSwap", c.pool))
            .and_then(|pool| {
                ctx.dex_addr("dex.factory", c.family.id)
                    .map(|f| (pool, f))
                    .map_err(|e| e.message)
            });
        if let Ok((pool, _)) = &plan {
            calls.push((*pool, P::factoryCall {}.abi_encode().into()));
            calls.push((*pool, P::tokenXCall {}.abi_encode().into()));
            calls.push((*pool, P::tokenYCall {}.abi_encode().into()));
            calls.push((*pool, P::feeCall {}.abi_encode().into()));
        }
        plans.push(plan);
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    // Первый проход: поля пула → (кандидат, пул, фабрика, найденный, fee) или итог сразу.
    let mut out: Vec<Option<Verdict>> = Vec::new();
    let mut second: Vec<(usize, Address, Found, (Address, Bytes))> = Vec::new();
    for (i, (c, plan)) in cands.iter().zip(plans).enumerate() {
        let (pool, factory) = match plan {
            Ok(p) => p,
            Err(w) => {
                out.push(Some(Verdict::Rejected(w)));
                continue;
            }
        };
        let part: Vec<call::Piece> = got.by_ref().take(PER).collect();
        let answers: Result<Vec<_>, String> = part.into_iter().collect();
        match answers {
            Err(node) => out.push(Some(Verdict::Failed(node))),
            Ok(a) => match fields(c, pool, factory, &a) {
                Ok((found, fee)) => {
                    let q = registry_call(c.family.proto, factory, found.token0, found.token1, fee);
                    second.push((i, pool, found, q));
                    out.push(None);
                }
                Err(w) => out.push(Some(Verdict::Rejected(w))),
            },
        }
    }
    let calls: Vec<(Address, Bytes)> = second.iter().map(|(_, _, _, q)| q.clone()).collect();
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    for ((i, pool, found, _), piece) in second.into_iter().zip(got) {
        out[i] = Some(match piece {
            Err(node) => Verdict::Failed(node),
            Ok(Ok(b)) => match registered(&found, pool, &b) {
                Ok(()) => Verdict::Taken(Box::new(found)),
                Err(w) => Verdict::Rejected(w),
            },
            Ok(Err(r)) => Verdict::Rejected(format!(
                "реестр фабрики iZiSwap откатил pool(x, y, fee): {r}"
            )),
        });
    }
    out.into_iter()
        .map(|v| v.unwrap_or_else(|| Verdict::Failed("iZiSwap: нет ответа реестра".into())))
        .collect()
}

/// Вызов реестра фабрики по виду: iZiSwap — pool(x, y, uint24), Biswap V3 — pool(x, y, uint16). Принимает: протокол,
/// фабрику, tokenX, tokenY, fee пула. Возвращает: (фабрика, данные вызова). Ошибок нет.
pub fn registry_call(
    p: Proto,
    factory: Address,
    x: Address,
    y: Address,
    fee: u32,
) -> (Address, Bytes) {
    let data = if p == Proto::BiswapV3 {
        BF::poolCall {
            tokenX: x,
            tokenY: y,
            fee: fee as u16,
        }
        .abi_encode()
    } else {
        IF::poolCall {
            tokenX: x,
            tokenY: y,
            fee: alloy_primitives::aliases::U24::from(fee),
        }
        .abi_encode()
    };
    (factory, data.into())
}

/// Решение по полям пула (без сети): factory, tokenX, tokenY, fee по порядку (откат — Err с текстом). Принимает:
/// кандидата, пул, фабрику DEX. Возвращает: найденный пул (пара по возрастанию = tokenX, tokenY) и fee или причину.
pub fn fields(
    c: &Candidate,
    pool: Address,
    factory: Address,
    answers: &[Result<Bytes, String>],
) -> Result<(Found, u32), String> {
    let at = |i: usize| match answers.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("пул iZiSwap {pool:#x} откатил вызов: {r}")),
        None => Err(format!("пул iZiSwap {pool:#x}: нет ответа")),
    };
    let addr = |i: usize, what: &str| -> Result<Address, String> {
        P::factoryCall::abi_decode_returns(at(i)?)
            .map_err(|e| format!("{what}() пула {pool:#x} не по ABI: {e}"))
    };
    let fa = addr(0, "factory")?;
    let x = addr(1, "tokenX")?;
    let y = addr(2, "tokenY")?;
    // fee — одно ABI-слово (uint24 у iZiSwap, uint16 у Biswap V3); значение больше uint24 — не по ABI.
    let w = at(3)?;
    let fee = (w.len() == 32)
        .then(|| U256::from_be_slice(w))
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v < (1 << 24))
        .ok_or_else(|| format!("fee() пула {pool:#x} не по ABI"))?;
    if fa != factory {
        return Err(format!(
            "фабрика пула {pool:#x} = {fa:#x}, у DEX {} — {factory:#x} (dex.factory)",
            c.family.id
        ));
    }
    let want = sorted(c.coin, c.quote);
    if (x, y) != want {
        return Err(format!(
            "токены пула iZiSwap {pool:#x} {x:#x} / {y:#x} ≠ монета {} и встречный {} (по возрастанию)",
            c.coin, c.quote
        ));
    }
    let found = Found {
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
    };
    Ok((found, fee))
}

/// Ответ реестра фабрики = сам пул. Принимает: найденный, пул, сырой ответ. Возвращает: () или причину.
pub fn registered(found: &Found, pool: Address, raw: &[u8]) -> Result<(), String> {
    let got = IF::poolCall::abi_decode_returns(raw)
        .map_err(|e| format!("реестр фабрики iZiSwap: ответ не по ABI: {e}"))?;
    if got != pool {
        return Err(format!(
            "пул {pool:#x} не из реестра фабрики (pool(x, y, fee) = {got:#x}) — dex.factory DEX {}",
            found.family.id
        ));
    }
    Ok(())
}
