//! Файл: venues/bsc_dex/check/hooks_judge.rs
//! Что делает: решение 32 × «335 (лесенка влияния)» — итог пробы влияния крючка B/C по ответам квотера (вынесено из
//! check/hooks_probe.rs ревью 4: файл был больше 200 строк; пути hooks_probe::{judge_plan, settled, impact} —
//! реэкспорт):
//! - judge_plan: влияние по точкам плана — математика пула без крючка на записи блока против ответа квотера;
//! - settled: итог замера цикла и перемера — пул «не прошла», только если не прошли все запрошенные стороны;
//! - impact: (математика − квотер) / математика · 100.
//!
//! Кто вызывает: check/hooks_probe.rs (measure_plan), check/hook_batch.rs, check/hooks_head.rs,
//! check/quoter_reprobe.rs; тесты на записях узла.
//! От чего зависит: check/hooks_probe.rs (Plan, Point, Measured, Quoted), check/hook_fold.rs (judge_side),
//! quote/pool_quote.rs, num.rs, core/types.
//! Настройки: нет.

use std::sync::Arc;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{HookImpact, HookStep};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::Entry;
use super::super::quote::pool_quote;
use super::hook_fold;
use super::hooks_probe::{Measured, Plan, Point, Quoted};

/// Что делает: влияние по точкам плана (см. шапку). Ревью 3: отказ квотера на точке (откат крючка, «не хватает
/// ликвидности») — эта точка и большие суммы стороны не измерены, меньшие работают (Б4, check/hook_fold.rs
/// judge_side); не сошлась наша математика — точка не измерена, отказом квотера не считается (Н-б). Сторона без
/// точек — buy_fail / sell_fail; отказы квотера — в refused. Пул целиком «не прошла» решает settled. Принимает:
/// окружение, запись на блоке квотера, план, ответы квотера по точкам (сначала покупка, затем продажа). Возвращает:
/// Measured. Ошибки: сбой узла в ответе (не откат), ответов меньше точек (BadResponse); математика пула.
pub fn judge_plan(ctx: &Ctx, e: &Entry, plan: &Plan, remote: Vec<Quoted>) -> Measured {
    let mut remote = remote.into_iter();
    type Got = (Vec<HookStep>, Vec<(U256, String)>);
    let mut side = |points: &[Point], token_in: Address| -> GatewayResult<Got> {
        let (mut out, mut refused) = (Vec::with_capacity(points.len()), Vec::new());
        for p in points {
            // Ревью Н10: квотер вернул 0 при ненулевой математике — крючок забрал всё: влияние 100 % (impact).
            let r = match remote.next() {
                Some(Ok(Ok(r))) => r,
                Some(Ok(Err(short))) => {
                    refused.push((p.amount_in, short));
                    continue;
                }
                Some(Err(err)) if err.code == ErrorCode::SwapReverted => {
                    refused.push((p.amount_in, err.message));
                    continue;
                }
                Some(Err(err)) => return Err(err),
                None => {
                    let why = "ответов квотера меньше, чем точек пробы";
                    return Err(ctx.err(ErrorCode::BadResponse, why));
                }
            };
            let local = pool_quote::swap(ctx, e, token_in, true, p.amount_in)?;
            if let (true, Some(pct)) = (local.complete, impact(local.amount_out, r)) {
                out.push(HookStep {
                    amount_in: p.amount_in,
                    usd: p.usd,
                    pct,
                    coins: None,
                    quoter_out: Some(r),
                });
            }
        }
        Ok((out, refused))
    };
    let (buy, buy_ref) = side(&plan.buy, e.usdt)?;
    let (sell, sell_ref) = side(&plan.sell, e.coin)?;
    let ours = "наша математика пула не сошлась на суммах пробы — влияние не определить";
    let fin = |steps: Vec<HookStep>, refused: &[(U256, String)], asked: bool| {
        let (kept, fail) = hook_fold::judge_side(steps, refused, asked);
        let fail = fail.or_else(|| (asked && kept.is_empty()).then(|| ours.to_string()));
        (kept, fail)
    };
    let (buy, buy_fail) = fin(buy, &buy_ref, !plan.buy.is_empty());
    let (sell, sell_fail) = fin(sell, &sell_ref, !plan.sell.is_empty());
    let top = |s: &[HookStep]| s.iter().map(|p| p.pct).max().unwrap_or(Decimal::ZERO);
    let tag = |b: bool, v: Vec<(U256, String)>| v.into_iter().map(move |(x, w)| (b, x, w));
    Ok(Ok(HookImpact {
        buy_pct: top(&buy),
        sell_pct: top(&sell),
        buy: Arc::new(buy),
        sell: Arc::new(sell),
        buy_fail,
        sell_fail,
        refused: tag(true, buy_ref).chain(tag(false, sell_ref)).collect(),
    }))
}

/// Что делает: итог замера для цикла крючков и перемера (ревью 3): все запрошенные стороны «не прошли» — пул «не
/// прошла» (причина первой стороны), иначе как есть (сторона без точек — только она). Принимает: итог, план. Возвращает:
/// Measured. Ошибок нет.
pub fn settled(m: Measured, plan: &Plan) -> Measured {
    let i = match m {
        Ok(Ok(i)) => i,
        other => return other,
    };
    let failed = |asked: bool, f: &Option<String>| !asked || f.is_some();
    let all = (!plan.buy.is_empty() || !plan.sell.is_empty())
        && failed(!plan.buy.is_empty(), &i.buy_fail)
        && failed(!plan.sell.is_empty(), &i.sell_fail);
    match (all, i.buy_fail.clone().or_else(|| i.sell_fail.clone())) {
        (true, Some(why)) => Ok(Err(why)),
        _ => Ok(Ok(i)),
    }
}

/// (математика − квотер) / математика · 100 (тесты на записях узла — та же формула). None — математика 0 или числа
/// не помещаются в Decimal.
pub fn impact(local: U256, remote: U256) -> Option<Decimal> {
    let l = num::to_decimal(local, 0)?;
    let r = num::to_decimal(remote, 0)?;
    if l.is_zero() {
        return None;
    }
    Some((l - r) / l * Decimal::ONE_HUNDRED)
}
