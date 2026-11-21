//! Файл: venues/bsc_dex/pools/verify_shell.rs
//! Что делает: решение 486 — проверка пула Shell Protocol (Proteus): адрес — в списке dex.stable_pools DEX (пулы из
//! официальной документации Shell; фабрики с реестром у Shell нет), учёт пула ocean() = dex.vault, стороны пула
//! (xToken / yToken — id в учёте) = монета и встречный токен (keccak256(токен, 0) или нативная монета учёта при
//! dex.native_wrapped), реализация implementation() — в списке сверенных (dex.proteus_utility_v1 / _v2). Пара записи —
//! по возрастанию адреса.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_shell.rs, shell_sides.rs, discover.rs (Found, sorted), verify.rs,
//! stable_native.rs.
//! Настройки: dex.stable_pools, dex.vault, dex.proteus_utility_v1 / _v2 [DEX], dex.native_wrapped [bsc_dex].
//!
//! Источник: docs.shellprotocol.io/start-here/reference-library/contract-addresses (Ocean, пулы Proteus Arbitrum);
//! Sourcify 42161 LiquidityPool.sol L29-L36, LiquidityPoolProxy.sol L16 (обе версии).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_shell::IShellPool as P;
use super::discover::{Found, sorted};
use super::shell_sides;
use super::stable_native;
use super::verify::{Candidate, Verdict};

/// Вызовов на кандидата: ocean, xToken, yToken, implementation.
const PER: usize = 4;

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (не в списке, нет dex.vault, не адрес —
/// отказ; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let wrapped = stable_native::wrapped(ctx);
    let mut plans: Vec<Result<(Address, Address), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = plan(ctx, c);
        if let Ok((pool, _)) = &plan {
            for d in [
                P::oceanCall {}.abi_encode(),
                P::xTokenCall {}.abi_encode(),
                P::yTokenCall {}.abi_encode(),
                P::implementationCall {}.abi_encode(),
            ] {
                calls.push((*pool, d.into()));
            }
        }
        plans.push(plan);
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let (pool, vault) = match plan {
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
            Ok(a) => match decide(ctx, c, pool, vault, wrapped, &a) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Адрес пула из списка dex.stable_pools и учёт dex.vault. Возвращает: (пул, учёт) или причину. Ошибок нет.
fn plan(ctx: &Ctx, c: &Candidate) -> Result<(Address, Address), String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула Shell", c.pool))?;
    let listed = ctx
        .dex_list("dex.stable_pools", c.family.id)
        .unwrap_or_default()
        .iter()
        .any(|s| Address::from_str(s.trim()).is_ok_and(|a| a == pool));
    if !listed {
        return Err(format!(
            "пул {pool:#x} не в dex.stable_pools DEX {} (адреса — из документации Shell)",
            c.family.id
        ));
    }
    let vault = ctx
        .dex_addr("dex.vault", c.family.id)
        .map_err(|e| e.message)?;
    Ok((pool, vault))
}

/// Решение по ответам ocean / xToken / yToken / implementation (порядок вызовов verify). Принимает: окружение (списки
/// реализаций), кандидата, пул, учёт DEX, обёртку, ответы. Возвращает: найденный пул или причину.
pub fn decide(
    ctx: &Ctx,
    c: &Candidate,
    pool: Address,
    vault: Address,
    wrapped: Option<Address>,
    answers: &[Result<Bytes, String>],
) -> Result<Found, String> {
    let at = |i: usize| match answers.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("пул Shell {pool:#x} откатил вызов: {r}")),
        None => Err(format!("пул Shell {pool:#x}: нет ответа")),
    };
    let dec =
        |what: &str, e: alloy::sol_types::Error| format!("{what} пула {pool:#x} не по ABI: {e}");
    let ocean = P::oceanCall::abi_decode_returns(at(0)?).map_err(|e| dec("ocean()", e))?;
    if ocean != vault {
        return Err(format!(
            "учёт пула {pool:#x} = {ocean:#x}, у DEX {} — {vault:#x} (dex.vault)",
            c.family.id
        ));
    }
    let x = P::xTokenCall::abi_decode_returns(at(1)?).map_err(|e| dec("xToken()", e))?;
    let y = P::yTokenCall::abi_decode_returns(at(2)?).map_err(|e| dec("yToken()", e))?;
    let imp = P::implementationCall::abi_decode_returns(at(3)?)
        .map_err(|e| dec("implementation()", e))?;
    shell_sides::sides(x, y, (c.coin, c.quote), wrapped)?;
    shell_sides::utility(ctx, c.family.id, imp)?;
    let want = sorted(c.coin, c.quote);
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
