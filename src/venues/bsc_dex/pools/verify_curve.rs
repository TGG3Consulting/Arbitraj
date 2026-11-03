//! Файл: venues/bsc_dex/pools/verify_curve.rs
//! Что делает: проверка пула StableSwap (Ellipsis StableSwap, шаблон Curve 2020; решение 122 F) из справочника на
//! своём узле. У базовых пулов Ellipsis нет фабрики с реестром (каждый пул развёрнут отдельно — ellipsis-finance/
//! ellipsis, contracts/StableSwap.vy, конструктор L120-L150), поэтому «кто подтверждает пул» (решение 59) — список
//! адресов пулов этой DEX из её официальной документации, который вводит Тигран: dex.stable_pools [DEX]. Пул
//! принимается, если он в этом списке И среди его монет (coins(k)) есть и монета, и встречный токен (CLAUDE.md:
//! адрес токена у биржи = адрес в пуле). Один проход Multicall3 частями по dex.verify_batch.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_curve.rs, load_curve.rs (coins_of), discover.rs, verify.rs.
//! Настройки: dex.stable_pools [DEX].

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_curve::{IStableSwapV1, MAX_COINS};
use super::discover::{Found, sorted};
use super::load_curve::coins_of;
use super::verify::{Candidate, Verdict};

/// Проверка пачки кандидатов StableSwap (MAX_COINS вызовов coins(k) на кандидата). Возвращает: итог по каждому.
/// Ошибок нет (не задан список или пул не в нём — отказ с причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Address, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = listed(ctx, c);
        if let Ok(pool) = plan {
            for k in 0..MAX_COINS {
                let data = IStableSwapV1::coinsCall { i: U256::from(k) }.abi_encode();
                calls.push((pool, data.into()));
            }
        }
        plans.push(plan);
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
        let part: Vec<call::Piece> = got.by_ref().take(MAX_COINS).collect();
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

/// Адрес пула кандидата, если он в dex.stable_pools этой DEX. Возвращает: адрес или причину отказа. Ошибок нет.
fn listed(ctx: &Ctx, c: &Candidate) -> Result<Address, String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула StableSwap", c.pool))?;
    let list = ctx
        .dex_list("dex.stable_pools", c.family.id)
        .map_err(|e| e.message)?;
    let known = list
        .iter()
        .filter_map(|s| Address::from_str(s.trim()).ok())
        .any(|a| a == pool);
    if !known {
        return Err(format!(
            "пул {pool:#x} не в dex.stable_pools DEX {} (адреса — из официальной документации DEX)",
            c.family.id
        ));
    }
    Ok(pool)
}

/// Решение по ответам coins(k) (без сети): обе монеты пары среди монет пула. Возвращает: найденный пул или причину.
pub fn decide(
    c: &Candidate,
    pool: Address,
    answers: &[Result<Bytes, String>],
) -> Result<Found, String> {
    let coins = coins_of(answers)?;
    if coins.len() < 2 {
        return Err(format!("у пула StableSwap {pool:#x} меньше двух монет"));
    }
    if !coins.contains(&c.coin) || !coins.contains(&c.quote) {
        return Err(format!(
            "среди монет пула StableSwap {pool:#x} нет пары {} / {}",
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
