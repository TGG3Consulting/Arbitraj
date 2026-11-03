//! Файл: venues/bsc_dex/exec/cycle_segment.rs
//! Что делает: шаги одного участка бандла цикла DEX ↔ DEX (решения 85, 86) — подряд идущие хопы ОДНОГО Universal Router
//! и их per-hop minOut (решение 40): ожидаемый выход × (1 − крючок) × (1 − exec.dex_slippage_pct). Участок считается
//! quote/route_calc.rs по своему фиксированному входу (как продажа цикла — каждый пул принимает точный вход): первый
//! хоп платит кошелёк этой суммой (Permit2), следующие — весь баланс роутера (CONTRACT_BALANCE), последний отдаёт
//! кошельку. Последнему хопу ПОСЛЕДНЕГО участка ставится граница ядра (решение 124): minOut = max(по допуску, floor) —
//! вход цикла + затраты + порог прибыли; бандл с меньшим выходом откатится целиком (атомарно, газ не горит).
//! Кто вызывает: exec/cycle_bundle.rs.
//! От чего зависит: quote/route_calc.rs, exec/{chain, plan, ur}.rs, num.rs, ctx.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex].

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::quote::route_calc::{self, Leg};
use super::chain::Step;
use super::plan::Order;
use super::ur::{self, CONTRACT_BALANCE};

/// Шаги участка и их minOut. Принимает: хопы участка, вход участка (единицы токена входа), границу выхода последнего
/// хопа (U256::ZERO — без границы; у последнего участка цикла — граница ядра). Возвращает: шаги, minOut по хопам.
/// Ошибки: NoRoute (пусто/шаг не исполним целиком), BadResponse (числа), QuoteMismatch (минимум хопа 0), NotConfigured
/// (exec.dex_slippage_pct), Internal (разные форматы UR).
pub fn steps(
    ctx: &Ctx,
    legs: &[Leg],
    input_units: U256,
    floor: U256,
) -> GatewayResult<(Vec<Step>, Vec<U256>)> {
    let first = legs
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "пустой участок цикла"))?;
    let input = num::to_decimal(input_units, first.dec_in).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "вход участка цикла не переводится в число",
        )
    })?;
    // Один формат входов UR на участок (как L3 exec/bundle.rs): plan::build кодирует все хопы форматом первого.
    let fmt = ur::format(ctx, &first.entry)?;
    for l in &legs[1..] {
        if ur::format(ctx, &l.entry)? != fmt {
            return Err(ctx.err(
                ErrorCode::Internal,
                "хопы участка цикла из разных экосистем (формат Universal Router не совпал)",
            ));
        }
    }
    let path = route_calc::eval(ctx, legs, Side::Sell, input, Decimal::ONE)?;
    let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let n = path.hops.len();
    let mut steps = Vec::with_capacity(n);
    let mut mins = Vec::with_capacity(n);
    for (j, h) in path.hops.iter().enumerate() {
        let keep = Decimal::ONE - h.usable.hook_pct / Decimal::ONE_HUNDRED;
        let mut min_out = num::to_units(h.pool_out * keep * slip, h.dec_out)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "минимум хопа цикла не считается"))?;
        if j + 1 == n {
            min_out = min_out.max(floor);
        }
        if min_out.is_zero() {
            let why = format!(
                "{}: минимум хопа цикла 0 — обмен без минимума не отправляется",
                h.entry.family.title
            );
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
        let from_wallet = j == 0;
        steps.push(Step {
            entry: h.entry.clone(),
            order: Order {
                token_in: h.token_in,
                token_out: h.token_out,
                amount_in: if from_wallet {
                    input_units
                } else {
                    CONTRACT_BALANCE
                },
                min_out,
            },
            from_wallet,
            to_wallet: j + 1 == n,
        });
        mins.push(min_out);
    }
    Ok((steps, mins))
}
