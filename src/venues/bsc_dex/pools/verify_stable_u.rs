//! Файл: venues/bsc_dex/pools/verify_stable_u.rs
//! Что делает: проверка пула из справочника (или поиска по фабрике) на своём узле (решение 59; решение 122 F, 3) для
//! PancakeSwap StableSwap, Lista StableSwap, Saddle (Nerve) и WooFi. Кто подтверждает пул:
//! • PancakeSwap — реестр фабрики V2 (dex.factory): getPairInfo(монета, встречный) или getThreePoolPairInfo вернул этот
//!   пул; пулы первого поколения (у фабрики V2 их нет) — список dex.stable_pools;
//! • Lista — реестр фабрики (dex.factory): getPairInfos(монета, встречный) содержит этот пул;
//! • Saddle — список dex.stable_pools (фабрики нет);
//! и у всех трёх обе монеты пары — среди монет пула (coins(k) / getToken(k); CLAUDE.md: адрес токена у биржи = в пуле).
//! • WooFi — пул = dex.pmm_pool, quoteToken() = встречный токен, wooracle() = dex.oracle, реализация прокси (слот
//!   EIP-1967) = dex.pool_implementation, оракул отдаёт состояние монеты (state(coin) не откатился, цена > 0).
//!   Ссылка — PoolRef::Member (контракт, монета).
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, verify_woo.rs, abi_curve.rs, abi_saddle.rs, abi_stable_u.rs, abi_woo.rs,
//! load_curve.rs (coins_of), load_saddle.rs (tokens_of), discover.rs, verify.rs.
//! Настройки: dex.factory, dex.stable_pools, dex.pmm_pool, dex.oracle, dex.pool_implementation [DEX].

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_curve::{IStableSwapV1, MAX_COINS};
use super::abi_saddle::{ISaddleSwap, MAX_TOKENS};
use super::abi_stable_u::{IListaStableFactory, IPancakeStableFactory};
use super::abi_woo::{IWooPP, IWooracle};
use super::discover::Found;
use super::families::Proto;
use super::load_curve::coins_of;
use super::load_saddle::tokens_of;
use super::verify::{Candidate, Verdict};
use super::verify_woo;

/// Проверка пачки кандидатов этих видов. Возвращает: итог по каждому. Ошибок нет (отказ — с причиной; узел — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut out = Vec::new();
    for c in cands {
        out.push(match one(ctx, c, size).await {
            Ok(Ok(f)) => Verdict::Taken(Box::new(f)),
            Ok(Err(why)) => Verdict::Rejected(why),
            Err(node) => Verdict::Failed(node),
        });
    }
    out
}

/// Один кандидат. Возвращает: Ok(Ok(пул)) — принят, Ok(Err) — отказ, Err — сбой узла.
async fn one(ctx: &Ctx, c: &Candidate, size: usize) -> Result<Result<Found, String>, String> {
    let pool = match Address::from_str(&c.pool) {
        Ok(a) => a,
        Err(_) => return Ok(Err(format!("«{}» — не адрес контракта пула", c.pool))),
    };
    let id = c.family.id;
    let (calls, n_coins) = match plan(ctx, c, pool) {
        Ok(p) => p,
        Err(why) => return Ok(Err(why)),
    };
    let got: Result<Vec<_>, String> = call::try_chunks(ctx, &calls, "latest", size)
        .await
        .into_iter()
        .collect();
    let got = got?;
    if c.family.proto == Proto::WooPP {
        return verify_woo::woo(ctx, c, pool, &got).await;
    }
    let coins = match c.family.proto {
        Proto::SaddleStable => tokens_of(&got[..n_coins]),
        _ => coins_of(&got[..n_coins]),
    };
    let coins = match coins {
        Ok(list) => list,
        Err(why) => return Ok(Err(why)),
    };
    if !coins.contains(&c.coin) || !coins.contains(&c.quote) {
        return Ok(Err(format!(
            "среди монет пула {pool:#x} нет пары {} / {}",
            c.coin, c.quote
        )));
    }
    let listed = ctx
        .dex_list("dex.stable_pools", id)
        .map(|l| {
            l.iter()
                .filter_map(|s| Address::from_str(s.trim()).ok())
                .any(|a| a == pool)
        })
        .unwrap_or(false);
    let reg = &got[n_coins..];
    let ok = match c.family.proto {
        Proto::SaddleStable => listed,
        Proto::PancakeStable => {
            let two = reg.first().and_then(|r| r.as_ref().ok()).and_then(|b| {
                IPancakeStableFactory::getPairInfoCall::abi_decode_returns(b)
                    .ok()
                    .map(|i| i.swapContract)
            });
            let three = reg.get(1).and_then(|r| r.as_ref().ok()).and_then(|b| {
                IPancakeStableFactory::getThreePoolPairInfoCall::abi_decode_returns(b)
                    .ok()
                    .map(|i| i.swapContract)
            });
            listed || two == Some(pool) || three == Some(pool)
        }
        Proto::ListaStable => reg
            .first()
            .and_then(|r| r.as_ref().ok())
            .and_then(|b| IListaStableFactory::getPairInfosCall::abi_decode_returns(b).ok())
            .is_some_and(|l| l.iter().any(|i| i.swapContract == pool)),
        _ => false,
    };
    if !ok {
        return Ok(Err(format!(
            "пул {pool:#x} не подтверждён: нет в реестре фабрики dex.factory и нет в dex.stable_pools DEX {id}"
        )));
    }
    Ok(Ok(verify_woo::found(c, PoolRef::Contract(pool))))
}

/// Вызовы проверки и число вызовов «монеты пула» в начале. Ошибка — причина отказа (не задана настройка).
fn plan(ctx: &Ctx, c: &Candidate, pool: Address) -> Result<(Vec<(Address, Bytes)>, usize), String> {
    let id = c.family.id;
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    match c.family.proto {
        Proto::WooPP => {
            let want = ctx.dex_addr("dex.pmm_pool", id).map_err(|e| e.message)?;
            if want != pool {
                return Err(format!(
                    "{pool:#x} — не общий контракт dex.pmm_pool DEX {id}"
                ));
            }
            let oracle = ctx.dex_addr("dex.oracle", id).map_err(|e| e.message)?;
            calls.push((pool, IWooPP::quoteTokenCall {}.abi_encode().into()));
            calls.push((pool, IWooPP::wooracleCall {}.abi_encode().into()));
            calls.push((
                oracle,
                IWooracle::stateCall { base: c.coin }.abi_encode().into(),
            ));
            return Ok((calls, 0));
        }
        Proto::SaddleStable => {
            for k in 0..MAX_TOKENS {
                calls.push((
                    pool,
                    ISaddleSwap::getTokenCall { index: k as u8 }
                        .abi_encode()
                        .into(),
                ));
            }
            return Ok((calls, MAX_TOKENS));
        }
        _ => {}
    }
    for k in 0..MAX_COINS {
        calls.push((
            pool,
            IStableSwapV1::coinsCall { i: U256::from(k) }
                .abi_encode()
                .into(),
        ));
    }
    let factory = ctx.dex_addr("dex.factory", id).map_err(|e| e.message)?;
    let (a, b) = (c.coin, c.quote);
    if c.family.proto == Proto::PancakeStable {
        calls.push((
            factory,
            IPancakeStableFactory::getPairInfoCall {
                _tokenA: a,
                _tokenB: b,
            }
            .abi_encode()
            .into(),
        ));
        let three = IPancakeStableFactory::getThreePoolPairInfoCall {
            _tokenA: a,
            _tokenB: b,
        };
        calls.push((factory, three.abi_encode().into()));
    } else {
        calls.push((
            factory,
            IListaStableFactory::getPairInfosCall {
                _tokenA: a,
                _tokenB: b,
            }
            .abi_encode()
            .into(),
        ));
    }
    Ok((calls, MAX_COINS))
}
