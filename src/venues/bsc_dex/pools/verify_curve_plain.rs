//! Файл: venues/bsc_dex/pools/verify_curve_plain.rs
//! Что делает: решение 472 — проверка пула Curve StableSwap 2021 (вариант CurvePlain вида curve_stableswap_plain) на
//! своём узле. Кто подтверждает пул (решение 59), по порядку:
//!
//! 1. Список dex.stable_pools DEX (пул без фабрики — 2pool Arbitrum 0x7f90…F353 из официального списка пулов Curve
//!    curvefi/curve-js ab3f1c2 src/constants/pools/arbitrum.ts L15-L20).
//! 2. Реестр фабрики Curve Sidechain/L2 (dex.factory): `get_implementation_address(pool)` — реализация, с исходником
//!    которой сверена математика: адрес из dex.curve_plain_implementations (у фабрики несколько реализаций на число
//!    монет — Basic, Balances, ETH, Optimized; формула совпадает только у сверенных). Ноль — пул не этой фабрики.
//!
//! И в обоих случаях — обе монеты пары среди coins(k) пула (метка ETH 0xEeee… — обёртка dex.native_wrapped;
//! verify_curve_plain_judge.rs). Один проход Multicall3 частями. Без dex.factory или без
//! dex.curve_plain_implementations путь 2 не работает (пул не из списка — отказ с причиной).
//! Решение 472 (доделка): метапул (вариант CurveMeta) — только путь 2 и ещё `get_base_pool(pool)` фабрики: базовый
//! пул должен быть coins(1) метапула (его get_virtual_price — курс монеты 1; load_curve_meta.rs).
//! Кто вызывает: pools/verify.rs (Proto::CurveStable с вариантом CurvePlain).
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_curve.rs, abi_curve_plain.rs, verify_curve_plain_judge.rs,
//! family_variant.rs.
//! Настройки: dex.stable_pools, dex.factory, dex.curve_plain_implementations [DEX].
//!
//! Источник реестра: curvefi/curve-factory fb61207 contracts/FactorySidechains.vy L451-L458 (Sourcify 42161 match
//! 0xb17b674D9c5CB2e441F8e196a2f048A81355d031; адрес фабрики — curve-js ab3f1c2 src/constants/network_constants.ts
//! L157 ALIASES_ARBITRUM.factory).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_curve::{IStableSwapV1, MAX_COINS};
use super::abi_curve_plain::ICurvePlainFactory;
use super::family_variant::Variant;
use super::verify::{Candidate, Verdict};
use super::verify_curve_plain_judge::{base_of, judge};

/// Кто подтверждает пул: список или фабрика (с её адресом, сверенными реализациями и признаком метапула).
enum Plan {
    Listed(Address),
    Factory(Address, Address, Vec<Address>, bool),
}

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (нет подтверждения — отказ с причиной; сбой
/// узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Plan, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = plan(ctx, c);
        match &plan {
            Ok(Plan::Listed(pool)) => push_coins(&mut calls, *pool),
            Ok(Plan::Factory(pool, factory, _, meta)) => {
                let q = ICurvePlainFactory::get_implementation_addressCall { pool: *pool };
                calls.push((*factory, q.abi_encode().into()));
                if *meta {
                    let q = ICurvePlainFactory::get_base_poolCall { pool: *pool };
                    calls.push((*factory, q.abi_encode().into()));
                }
                push_coins(&mut calls, *pool);
            }
            Err(_) => {}
        }
        plans.push(plan);
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let (pool, allowed, meta) = match plan {
            Ok(Plan::Listed(p)) => (p, None, false),
            Ok(Plan::Factory(p, _, list, meta)) => (p, Some(list), meta),
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let imp = allowed.map(|list| (got.next(), list));
        let base = if meta { Some(base_of(got.next())) } else { None };
        let part: Vec<call::Piece> = got.by_ref().take(MAX_COINS).collect();
        if let Some((answer, list)) = imp {
            match implementation(answer, &list, pool) {
                Ok(()) => {}
                Err(Some(node)) => {
                    out.push(Verdict::Failed(node));
                    continue;
                }
                Err(None) => {
                    out.push(Verdict::Rejected(reject(pool, c.family.id)));
                    continue;
                }
            }
        }
        let base = match base {
            None => None,
            Some(Ok(b)) => Some(b),
            Some(Err(Some(node))) => {
                out.push(Verdict::Failed(node));
                continue;
            }
            Some(Err(None)) => {
                let w = format!("пул {pool:#x}: фабрика не знает его базового пула — не метапул фабрики");
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let answers: Result<Vec<_>, String> = part.into_iter().collect();
        out.push(match answers {
            Err(node) => Verdict::Failed(node),
            Ok(a) => judge(ctx, c, pool, &a, base),
        });
    }
    out
}

/// Вызовы coins(k), k < MAX_COINS (первый откат — конец списка монет). Ошибок нет.
fn push_coins(calls: &mut Vec<(Address, Bytes)>, pool: Address) {
    for k in 0..MAX_COINS {
        let data = IStableSwapV1::coinsCall { i: U256::from(k) }.abi_encode();
        calls.push((pool, data.into()));
    }
}

/// План проверки кандидата: в dex.stable_pools — список (не у метапула: его базовый пул подтверждает только фабрика);
/// иначе фабрика, если заданы dex.factory и dex.curve_plain_implementations. Возвращает: план или причину отказа.
/// Ошибок нет.
fn plan(ctx: &Ctx, c: &Candidate) -> Result<Plan, String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула Curve", c.pool))?;
    let id = c.family.id;
    let meta = c.family.variant == Variant::CurveMeta;
    let listed = ctx.dex_list("dex.stable_pools", id).unwrap_or_default();
    let known = listed
        .iter()
        .filter_map(|s| Address::from_str(s.trim()).ok())
        .any(|a| a == pool);
    if known && !meta {
        return Ok(Plan::Listed(pool));
    }
    let factory = ctx.dex_addr("dex.factory", id).map_err(|_| reject(pool, id))?;
    let imps: Vec<Address> = ctx
        .dex_list("dex.curve_plain_implementations", id)
        .map_err(|_| reject(pool, id))?
        .iter()
        .filter_map(|s| Address::from_str(s.trim()).ok())
        .collect();
    if imps.is_empty() {
        return Err(reject(pool, id));
    }
    Ok(Plan::Factory(pool, factory, imps, meta))
}

/// Ответ get_implementation_address фабрики: Ok — реализация из списка; Err(None) — не из списка или ноль (пул не
/// фабрики); Err(Some) — сбой узла / ответ не по ABI. Ошибок иных нет.
fn implementation(
    answer: Option<call::Piece>,
    list: &[Address],
    pool: Address,
) -> Result<(), Option<String>> {
    let bytes = match answer {
        Some(Ok(Ok(b))) => b,
        Some(Ok(Err(_))) => return Err(None),
        Some(Err(node)) => return Err(Some(node)),
        None => return Err(Some(format!("нет ответа фабрики по пулу {pool:#x}"))),
    };
    let imp = ICurvePlainFactory::get_implementation_addressCall::abi_decode_returns(&bytes)
        .map_err(|e| Some(format!("ответ get_implementation_address не по ABI: {e}")))?;
    if list.contains(&imp) {
        Ok(())
    } else {
        Err(None)
    }
}

/// Текст отказа: пула нет в списке и фабрика его не подтвердила сверенной реализацией. Ошибок нет.
fn reject(pool: Address, id: &str) -> String {
    format!(
        "пул {pool:#x} не в dex.stable_pools DEX {id} и не подтверждён фабрикой (dex.factory, реализация пула — из \
         dex.curve_plain_implementations)"
    )
}
