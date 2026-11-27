//! Файл: venues/bsc_dex/pools/verify_lb20.rs
//! Что делает: решение 445 — проверка пула Liquidity Book v2.0 (LFJ V2, Arbitrum) из справочника на своём узле. То же
//! правило, что у v2.1 (verify_lb.rs, решение 59): (1) пул называет фабрику factory() = dex.factory (immutable
//! реализации — его назовёт и чужой клон, поэтому одного его мало); (2) монета и встречный — токены пула X и Y
//! (CLAUDE.md: адрес у биржи = адрес в пуле); (3) реестр фабрики getLBPairInformation(X, Y, binStep пула) отдаёт этот
//! же адрес — запись делает только createLBPair фабрики. Отличие v2.0 — геттеры пула: factory(), tokenX(), tokenY(),
//! шаг бина — поле binStep структуры feeParameters(). Второй проход и решение — те же функции v2.1 (verify_lb.rs
//! second_call, decide: подпись и ответ getLBPairInformation у фабрик v2.0 и v2.1 одинаковы, токены сортируются).
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_lb20.rs, verify_lb.rs, discover.rs, verify.rs.
//! Настройки: dex.factory [DEX вида lb_v20].
//!
//! Источник: lfj-gg/joe-v2, тег v2.0.0 (07f10212) — src/LBPair.sol L51 (factory: immutable), L56-L59 (tokenX, tokenY —
//! пишет initialize L131-L132), L249-L251 (feeParameters); src/LBFactory.sol L105-L111, L597-L614
//! (getLBPairInformation: _sortTokens), L237-L297 (createLBPair: клон Clones.cloneDeterministic, запись реестра);
//! src/interfaces/ILBFactory.sol L19-L24 (LBPairInformation).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_lb20::ILBPairV20 as P;
use super::verify::{Candidate, Verdict};
use super::verify_lb::{self, Fields, Plan};

/// Вызовов первого прохода на пул (factory, tokenX, tokenY, feeParameters).
pub const FIRST: usize = 4;

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
            calls.push(verify_lb::second_call(p, f));
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
                Some(Ok(a)) => match verify_lb::decide(c, p, f, &a) {
                    Ok(found)
                        if found.family.variant == super::family_variant::Variant::SectorLb =>
                    {
                        verify_lb::verify_clone(ctx, found, p.0).await
                    }
                    Ok(found) => Verdict::Taken(Box::new(found)),
                    Err(w) => Verdict::Rejected(w),
                },
            },
        });
    }
    out
}

/// Адреса проверки кандидата. Возвращает: план или причину отказа (не адрес, не задана dex.factory).
fn plan(ctx: &Ctx, c: &Candidate) -> Result<Plan, String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула LB v2.0", c.pool))?;
    let factory = ctx
        .dex_addr("dex.factory", c.family.id)
        .map_err(|e| e.message)?;
    Ok((pool, factory))
}

/// Вызовы первого прохода к пулу: factory, tokenX, tokenY, feeParameters. Ошибок нет.
pub fn first_calls(pool: Address) -> Vec<(Address, Bytes)> {
    [
        P::factoryCall {}.abi_encode(),
        P::tokenXCall {}.abi_encode(),
        P::tokenYCall {}.abi_encode(),
        P::feeParametersCall {}.abi_encode(),
    ]
    .into_iter()
    .map(|d| (pool, Bytes::from(d)))
    .collect()
}

/// Первый проход (без сети): фабрика пула = dex.factory, монета и встречный — токены X и Y. Принимает: кандидата,
/// план, ответы по порядку first_calls (откат — Err с текстом). Возвращает: (X, Y, шаг) или причину отказа.
pub fn first_pass(c: &Candidate, p: &Plan, a: &[Result<Bytes, String>]) -> Result<Fields, String> {
    let (pool, want) = *p;
    let at = |i: usize, what: &str| match a.get(i) {
        Some(Ok(b)) => Ok(b.clone()),
        Some(Err(r)) => Err(format!(
            "пул {pool:#x} откатил {what}: {r} — не пул Liquidity Book v2.0"
        )),
        None => Err(format!("нет ответа пула {pool:#x} на {what}")),
    };
    let f = P::factoryCall::abi_decode_returns(&at(0, "factory()")?)
        .map_err(|e| format!("factory() не по ABI: {e}"))?;
    if f != want {
        return Err(format!(
            "пул {pool:#x} называет фабрику {f:#x}, а dex.factory — {want:#x}"
        ));
    }
    let x = P::tokenXCall::abi_decode_returns(&at(1, "tokenX()")?)
        .map_err(|e| format!("tokenX(): {e}"))?;
    let y = P::tokenYCall::abi_decode_returns(&at(2, "tokenY()")?)
        .map_err(|e| format!("tokenY(): {e}"))?;
    let fp = P::feeParametersCall::abi_decode_returns(&at(3, "feeParameters()")?)
        .map_err(|e| format!("feeParameters(): {e}"))?;
    let pair_ok = (x == c.coin && y == c.quote) || (x == c.quote && y == c.coin);
    if !pair_ok {
        return Err(format!(
            "токены пула LB v2.0 {pool:#x} — {x:#x} / {y:#x}, а не {} / {}",
            c.coin, c.quote
        ));
    }
    Ok((x, y, fp.binStep))
}
