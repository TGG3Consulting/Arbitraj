//! Файл: core/refit.rs
//! Что делает: объём под свободный остаток (решение 103). Прямо перед ордером покупки бот один раз читает свободный
//! баланс котируемой монеты на A (решение 94, limits::buy_cap). Если списание плана по худшей цене больше свободного —
//! раньше покупка отклонялась («… больше бюджета 19.49, ордер не ставится»); теперь объём подбирается заново тем же
//! расчётом (profit::decide_capped: подбор объёма, все затраты, финальная котировка исполнителя, пороги
//! money.min_profit_usd и money.min_profit_pct) с бюджетом, равным свободному остатку. Прибыль на меньшем объёме
//! выше порогов — покупка по новому плану; нет — отказ с причиной, ордер не ставится. Повторного чтения баланса нет.
//! Кто вызывает: core/executor.rs.
//! От чего зависит: profit.rs, costs.rs (Calc), dry.rs (режим Live — деньги настоящие).
//! Настройки: нет своих (всё, что читает profit.rs).

use rust_decimal::Decimal;

use crate::core::types::Direction;

use super::costs::Calc;
use super::engine::Core;
use super::profit::{self, Verdict};

/// Нужно ли подбирать объём заново: списание плана по худшей цене больше предела. Ошибок нет.
pub fn needed(plan: &Calc, cap: Decimal) -> bool {
    plan.spend_max > cap
}

/// Новый план под предел `cap`. Возвращает: расчёт (выше порогов, списание ≤ cap). Ошибки: текст отказа расчёта.
pub async fn plan(core: &Core, d: &Direction, cap: Decimal) -> Result<Calc, String> {
    match profit::decide_capped(core, d, &super::dry::Mode::Live, Some(cap)).await {
        Verdict::Accept(c) if c.spend_max <= cap => Ok(c),
        Verdict::Accept(c) => Err(format!(
            "списание по худшей цене {} больше свободного {cap}",
            c.spend_max.round_dp(4)
        )),
        Verdict::Reject { reason, .. } => Err(reason),
    }
}
