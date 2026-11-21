//! Файл: venues/bsc_dex/quote/bound_cycle.rs
//! Что делает: решение 373 — строгий верх отдачи цикла DEX ↔ DEX: ρ = произведение r_e ног (quote/bound_rate.rs; каждый
//! пул в цикле один раз — route_paths.rs enumerate_cycles, путь один — без деления, quote/cycle_opt.rs) и отсев цикла:
//! ρ < 1 и dex_dex.min_profit_base ≥ 0 ⇒ выход < вход на любом входе, газ ≥ 0 ⇒ чистая прибыль < 0 ≤ порога.
//! Доказательство — docs/notes/t162-proven-sieve-2026-11-17.md, раздел 2.
//! Решение 385: проверка отдаёт ρ (cycle_sieve) — по нему контроль отсева (quote/cycle_audit.rs) сверяет полный счёт.
//! Кто вызывает: quote/cycle_scan.rs (signal), тесты.
//! От чего зависит: quote/bound_rate.rs, quote/route_calc.rs (Leg), gateway/rate_bound.rs (счётчики), ctx.rs.
//! Настройки: dex_dex.min_profit_base [bsc_dex]; через bound_rate — dex.v2_fee_pct [вид пула V2].

use rust_decimal::Decimal;

use super::super::ctx::Ctx;
use super::bound_rate::{ceil_dec, mul_up, rate};
use super::route_calc::Leg;

/// Решение 373: верх отдачи цикла DEX ↔ DEX — произведение r_e ног (каждый пул в цикле один раз, путь один — без
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

/// Итог проверки цикла верхом отдачи (решения 373, 385).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CycleSieve {
    /// Верх строгий и не исключает прибыль — цикл считается.
    Pass,
    /// Отсечён: строгий верх ρ < 1 и порог dex_dex.min_profit_base ≥ 0 — прибыли не ниже порога нет ни на каком входе.
    Cut(Decimal),
    /// Метод не действует (нога без строгого r_e) — цикл считается, как раньше.
    Na,
}

/// Что делает: решения 373, 385 — проверка цикла верхом отдачи с отметкой счётчика (gateway/rate_bound.rs cycle_count):
/// отсечён, только если порог ≥ 0 и строгий верх ρ цикла < 1 (выход ≤ ρ·вход < вход, газ ≥ 0). ρ отдаётся вызывающему —
/// контроль отсева (quote/cycle_audit.rs) сверяет полный счёт с ним на тех же ногах (тот же снимок пулов).
/// Принимает: окружение, ноги цикла. Возвращает: CycleSieve. Ошибок нет (верх не строгий — Na).
pub fn cycle_sieve(ctx: &Ctx, legs: &[Leg]) -> CycleSieve {
    let floor = ctx.dec("dex_dex.min_profit_base").unwrap_or(Decimal::ZERO);
    match cycle_rho(ctx, legs) {
        Ok(rho) if rho < Decimal::ONE && !floor.is_sign_negative() => {
            crate::gateway::rate_bound::cycle_count(1);
            CycleSieve::Cut(rho)
        }
        Ok(_) => {
            crate::gateway::rate_bound::cycle_count(0);
            CycleSieve::Pass
        }
        Err(_) => {
            crate::gateway::rate_bound::cycle_count(2);
            CycleSieve::Na
        }
    }
}

/// Что делает: решение 373 — может ли цикл дать прибыль не ниже dex_dex.min_profit_base (cycle_sieve: не Cut).
/// Принимает: окружение, ноги цикла. Возвращает: false — цикл отсечён. Ошибок нет (верх не строгий — true).
pub fn cycle_may_profit(ctx: &Ctx, legs: &[Leg]) -> bool {
    !matches!(cycle_sieve(ctx, legs), CycleSieve::Cut(_))
}
