//! Файл: venues/bsc_dex/quote/bound_cycle.rs
//! Что делает: решение 362 — строгий верх отдачи цикла DEX ↔ DEX: ρ = произведение r_e ног (quote/bound_rate.rs; каждый
//! пул в цикле один раз — route_paths.rs enumerate_cycles, путь один — без деления, quote/cycle_opt.rs) и отсев цикла:
//! ρ < 1 и dex_dex.min_profit_base ≥ 0 ⇒ выход < вход на любом входе, газ ≥ 0 ⇒ чистая прибыль < 0 ≤ порога.
//! Доказательство — docs/notes/t162-proven-sieve-2026-11-17.md, раздел 2.
//! Кто вызывает: quote/cycle_scan.rs (signal).
//! От чего зависит: quote/bound_rate.rs, quote/route_calc.rs (Leg), gateway/rate_bound.rs (счётчики), ctx.rs.
//! Настройки: dex_dex.min_profit_base [bsc_dex]; через bound_rate — dex.v2_fee_pct [вид пула V2].

use rust_decimal::Decimal;

use super::super::ctx::Ctx;
use super::bound_rate::{ceil_dec, mul_up, rate};
use super::route_calc::Leg;

/// Решение 362: верх отдачи цикла DEX ↔ DEX — произведение r_e ног (каждый пул в цикле один раз, путь один — без
/// деления, quote/cycle_opt.rs). Принимает: окружение, ноги цикла. Возвращает: ρ цикла (выход ≤ ρ·вход); Err — нога без
/// строгого r_e (метод не действует, цикл считается как раньше).
pub fn cycle_rho(ctx: &Ctx, legs: &[Leg]) -> Result<Decimal, String> {
    let mut rho = Decimal::ONE;
    for l in legs {
        let r =
            ceil_dec(rate(ctx, &l.entry, l.token_in)?).ok_or("верх пула не помещается в число")?;
        rho = mul_up(rho, r).ok_or("верх цикла не помещается в число")?;
    }
    Ok(rho)
}

/// Что делает: решение 362 — может ли цикл дать прибыль не ниже dex_dex.min_profit_base: нет, только если порог ≥ 0 и
/// строгий верх ρ цикла < 1 (выход ≤ ρ·вход < вход, газ ≥ 0). Отмечает счётчик (gateway/rate_bound.rs cycle_count).
/// Принимает: окружение, ноги цикла. Возвращает: false — цикл отсечён. Ошибок нет (верх не строгий — true).
pub fn cycle_may_profit(ctx: &Ctx, legs: &[Leg]) -> bool {
    let floor = ctx.dec("dex_dex.min_profit_base").unwrap_or(Decimal::ZERO);
    match cycle_rho(ctx, legs) {
        Ok(rho) if rho < Decimal::ONE && !floor.is_sign_negative() => {
            crate::gateway::rate_bound::cycle_count(1);
            false
        }
        Ok(_) => {
            crate::gateway::rate_bound::cycle_count(0);
            true
        }
        Err(_) => {
            crate::gateway::rate_bound::cycle_count(2);
            true
        }
    }
}
