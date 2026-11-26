//! Файл: venues/bsc_dex/pools/verify_lb.rs
//! Что делает: проверка пула Liquidity Book V2.1 (решение 290) из справочника на своём узле. Кто подтверждает пул
//! (решение 59): (1) пул называет фабрику getFactory() = dex.factory (поле неизменяемое у реализации — его же назовёт и
//! чужой клон той же реализации, поэтому одного его мало); (2) монета и встречный — это токены пула X и Y (CLAUDE.md:
//! адрес токена у биржи = адрес в пуле); (3) реестр фабрики getLBPairInformation(X, Y, binStep пула) отдаёт этот же
//! адрес: запись делает только createLBPair фабрики — самозванец (свой клон, пул другой фабрики) отсекается. Пометка
//! ignoredForRouting — подсказка роутеру LFJ, на обмен не влияет: пул не отклоняет. Два прохода частями по
//! dex.verify_batch.
//! Кто вызывает: pools/verify.rs (lb_v21), pools/verify_lb22.rs (lb_v22, решение 446: первые два прохода те же).
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_lb.rs, discover.rs, verify.rs.
//! Настройки: dex.factory [DEX вида lb_v21].
//!
//! Источник: lfj-gg/joe-v2@v2.1.0 — src/LBPair.sol L124-L152 (getFactory: immutable _factory; getTokenX / getTokenY /
//! getBinStep — аргументы клона), src/LBFactory.sol L170-L177, L607-L614 (getLBPairInformation, токены сортируются),
//! src/interfaces/ILBFactory.sol L42-L47.

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_lb::{ILBFactory as F, ILBPair as P};
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Адреса проверки: пул и фабрика из настройки.
pub type Plan = (Address, Address);

/// Что дал первый проход: токен X, токен Y, шаг бина.
pub type Fields = (Address, Address, u16);

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (не задана настройка, пул не прошёл — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let plans: Vec<Result<Plan, String>> = cands.iter().map(|c| plan(ctx, c)).collect();
    let calls: Vec<(Address, Bytes)> = plans
        .iter()
        .flatten()
        .flat_map(|p| first_calls(p.0))
        .collect();
    let first = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut it = first.chunks(FIRST);
    let mut fields: Vec<Result<Fields, Verdict>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for (c, p) in cands.iter().zip(&plans) {
        let Ok(p) = p else {
            fields.push(Err(Verdict::Rejected(String::new())));
            continue;
        };
        let part: &[call::Piece] = it.next().unwrap_or(&[]);
        let answers: Result<Vec<Result<Bytes, String>>, String> = part.iter().cloned().collect();
        let r = match answers {
            Err(node) => Err(Verdict::Failed(node)),
            Ok(a) if a.len() != FIRST => Err(Verdict::Failed("нет ответа узла".into())),
            Ok(a) => first_pass(c, p, &a).map_err(Verdict::Rejected),
        };
        if let Ok(f) = &r {
            calls.push(second_call(p, f));
        }
        fields.push(r);
    }
    let second = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut second = second.into_iter();
    let mut out = Vec::new();
    for ((c, p), f) in cands.iter().zip(plans).zip(fields) {
        out.push(match (p, f) {
            (Err(w), _) => Verdict::Rejected(w),
            (_, Err(v)) => v,
            (Ok(p), Ok(f)) => match second.next() {
                None => Verdict::Failed("нет ответа фабрики".into()),
                Some(Err(node)) => Verdict::Failed(node),
                Some(Ok(a)) => match decide(c, p, f, &a) {
                    Ok(found) if c.family.variant == super::family_variant::Variant::LbV21Clone => {
                        verify_clone(ctx, found, p.0).await
                    }
                    Ok(found) => Verdict::Taken(Box::new(found)),
                    Err(w) => Verdict::Rejected(w),
                },
            },
        });
    }
    out
}

/// LFJ ImmutableClone (v2.1.0): фиксированный код делегирования, за ним tokenX, tokenY, binStep и размер аргументов.
pub fn clone_implementation(code: &[u8]) -> Option<Address> {
    const PREFIX: [u8; 20] = alloy_primitives::hex!("363d3d373d3d3d3d61002c806035363936013d73");
    const SUFFIX: [u8; 13] = alloy_primitives::hex!("5af43d3d93803e603357fd5bf3");
    if code.len() != 97 || code[..20] != PREFIX || code[40..53] != SUFFIX || code[95..] != [0, 44] {
        return None;
    }
    Some(Address::from_slice(&code[20..40]))
}

async fn verify_clone(ctx: &Ctx, found: Found, pool: Address) -> Verdict {
    let implementations = match super::verify_lb22::settings(ctx, found.family) {
        Ok(items) => items,
        Err(w) => return Verdict::Rejected(w),
    };
    let reply: Result<Bytes, _> =
        super::super::node::rpc::call(ctx, "eth_getCode", serde_json::json!([pool, "latest"]))
            .await;
    match reply {
        Err(e) => Verdict::Failed(e.message),
        Ok(code) if clone_implementation(&code).is_some_and(|a| implementations.contains(&a)) => {
            Verdict::Taken(Box::new(found))
        }
        Ok(_) => Verdict::Rejected(format!(
            "шаблон клона LB {pool:#x} не подтверждён dex.lb_implementations"
        )),
    }
}

/// Вызовов первого прохода на пул (getFactory, getTokenX, getTokenY, getBinStep).
pub const FIRST: usize = 4;

/// Адреса проверки кандидата. Возвращает: план или причину отказа (не адрес, не задана dex.factory).
fn plan(ctx: &Ctx, c: &Candidate) -> Result<Plan, String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула LB", c.pool))?;
    let factory = ctx
        .dex_addr("dex.factory", c.family.id)
        .map_err(|e| e.message)?;
    Ok((pool, factory))
}

/// Вызовы первого прохода к пулу: getFactory, getTokenX, getTokenY, getBinStep. Ошибок нет.
pub fn first_calls(pool: Address) -> Vec<(Address, Bytes)> {
    [
        P::getFactoryCall {}.abi_encode(),
        P::getTokenXCall {}.abi_encode(),
        P::getTokenYCall {}.abi_encode(),
        P::getBinStepCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (pool, Bytes::from(d)))
    .collect()
}

/// Версия вида кандидата для текстов отказа (ревью 446, Н5): V2.2 у lb_v22, иначе V2.1. Ошибок нет.
fn version(c: &Candidate) -> &'static str {
    if c.family.proto == super::families::Proto::LbV22 {
        "V2.2"
    } else {
        "V2.1"
    }
}

/// Первый проход (без сети): фабрика пула = dex.factory, монета и встречный — токены X и Y. Принимает: кандидата,
/// план, ответы по порядку first_calls (откат — Err с текстом). Возвращает: (X, Y, шаг) или причину отказа.
pub fn first_pass(c: &Candidate, p: &Plan, a: &[Result<Bytes, String>]) -> Result<Fields, String> {
    let (pool, want) = *p;
    let at = |i: usize, what: &str| match a.get(i) {
        Some(Ok(b)) => Ok(b.clone()),
        Some(Err(r)) => Err(format!(
            "пул {pool:#x} откатил {what}: {r} — не пул Liquidity Book {}",
            version(c)
        )),
        None => Err(format!("нет ответа пула {pool:#x} на {what}")),
    };
    let f = P::getFactoryCall::abi_decode_returns(&at(0, "getFactory()")?)
        .map_err(|e| format!("getFactory() не по ABI: {e}"))?;
    if f != want {
        return Err(format!(
            "пул {pool:#x} называет фабрику {f:#x}, а dex.factory — {want:#x}"
        ));
    }
    let x = P::getTokenXCall::abi_decode_returns(&at(1, "getTokenX()")?)
        .map_err(|e| format!("getTokenX(): {e}"))?;
    let y = P::getTokenYCall::abi_decode_returns(&at(2, "getTokenY()")?)
        .map_err(|e| format!("getTokenY(): {e}"))?;
    let step = P::getBinStepCall::abi_decode_returns(&at(3, "getBinStep()")?)
        .map_err(|e| format!("getBinStep(): {e}"))?;
    let pair_ok = (x == c.coin && y == c.quote) || (x == c.quote && y == c.coin);
    if !pair_ok {
        return Err(format!(
            "токены пула LB {pool:#x} — {x:#x} / {y:#x}, а не {} / {}",
            c.coin, c.quote
        ));
    }
    Ok((x, y, step))
}

/// Вызов второго прохода: getLBPairInformation(X, Y, шаг) фабрики. Ошибок нет.
pub fn second_call(p: &Plan, f: &Fields) -> (Address, Bytes) {
    let c = F::getLBPairInformationCall {
        tokenA: f.0,
        tokenB: f.1,
        binStep: U256::from(f.2),
    };
    (p.1, Bytes::from(c.abi_encode()))
}

/// Решение по ответу реестра (без сети). Принимает: кандидата, план, поля пула, ответ getLBPairInformation.
/// Возвращает: найденный пул или причину отказа (в реестре под этой парой и шагом другой адрес — самозванец).
pub fn decide(
    c: &Candidate,
    p: Plan,
    f: Fields,
    a: &Result<Bytes, String>,
) -> Result<Found, String> {
    let (pool, factory) = p;
    let b = a
        .as_ref()
        .map_err(|r| format!("фабрика LB {factory:#x} откатила getLBPairInformation: {r}"))?;
    let info = F::getLBPairInformationCall::abi_decode_returns(b)
        .map_err(|e| format!("getLBPairInformation не по ABI: {e}"))?;
    if info.LBPair != pool {
        return Err(format!(
            "реестр фабрики {factory:#x} знает под парой и шагом {} пул {:#x}, а не {pool:#x} — пул-самозванец",
            f.2, info.LBPair
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
        bin_step: Some(u32::from(f.2)),
        key: None,
        hooks: None,
    })
}
