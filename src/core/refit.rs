//! Файл: core/refit.rs
//! Что делает: объём под свободный остаток (решение 103). Прямо перед ордером покупки бот один раз читает свободный
//! баланс котируемой монеты на A (решение 94, limits::buy_cap). Если списание плана по худшей цене больше свободного —
//! раньше покупка отклонялась («… больше бюджета 19.49, ордер не ставится»); теперь объём подбирается заново тем же
//! расчётом (profit::decide_capped: подбор объёма, все затраты, финальная котировка исполнителя, пороги
//! money.min_profit_usd и money.min_profit_pct) с бюджетом, равным свободному остатку. Прибыль на меньшем объёме
//! выше порогов — покупка по новому плану; нет — отказ с причиной, ордер не ставится. Повторного чтения баланса нет.
//! Ревью t166 (Н2, решение 380): объём продажи нового плана — не больше проверенного предпроверкой (limit →
//! DirCtx.sell_max): refit не продаёт больше, чем доказала симуляция продажи DEX или проверка ордера биржи B.
//! Кто вызывает: core/executor.rs.
//! От чего зависит: profit.rs, costs.rs (Calc), dry.rs (режим Live — деньги настоящие).
//! Настройки: нет своих (всё, что читает profit.rs).

use rust_decimal::Decimal;

use crate::core::types::Direction;

use super::costs::Calc;
use super::engine::Core;
use super::profit::{self, Verdict};

/// Предел продажи подбора под остаток (ревью t166, Н2): не больше объёма продажи плана, прошедшего предпроверку
/// (симуляция продажи DEX на точном объёме — решения 234, 380; проверка ордера биржи B — решение 134). Без него при
/// упавшей цене тот же предел списания покупает больше монет, и продажа ушла бы на непроверенном объёме. Принимает:
/// контекст направления, проверенный план. Ошибок нет.
pub fn limit(cx: &mut super::dirctx::DirCtx, checked: &Calc) {
    cx.sell_max = Some(
        cx.sell_max
            .map_or(checked.sell_qty, |m| m.min(checked.sell_qty)),
    );
}

/// Нужно ли подбирать объём заново: списание плана по худшей цене больше предела. Ошибок нет.
pub fn needed(plan: &Calc, cap: Decimal) -> bool {
    plan.spend_max > cap
}

/// Новый план под предел `cap`. Принимает: ядро, направление, предел списания, план предпроверки (его объём продажи —
/// проверенный: симуляция продажи DEX / проверка ордера биржи B — предел продажи нового плана, ревью t166 Н2).
/// Возвращает: расчёт (выше порогов, списание ≤ cap, продажа ≤ продажи плана). Ошибки: текст отказа расчёта.
pub async fn plan(
    core: &Core,
    d: &Direction,
    cap: Decimal,
    checked: &Calc,
) -> Result<Calc, String> {
    let mut cx = super::dirctx::DirCtx::build(core, d)?;
    limit(&mut cx, checked);
    // Решение 348: назначение «предпроверка» (замер «кто вызвал котировку»); решение 345 — без мягкого срока.
    let work = profit::decide_capped(
        core,
        d,
        &super::dry::Mode::Live,
        Some(cap),
        None,
        Some(Box::new(cx)),
        None,
    );
    let purpose = crate::gateway::quote_purpose::Purpose::Precheck;
    match crate::gateway::quote_purpose::scope(purpose, work).await {
        Verdict::Accept(c) if c.spend_max <= cap && c.sell_qty <= checked.sell_qty => Ok(c),
        Verdict::Accept(c) if c.spend_max <= cap => Err(format!(
            "продажа {} больше проверенной предпроверкой {}",
            c.sell_qty.normalize(),
            checked.sell_qty.normalize()
        )),
        Verdict::Accept(c) => Err(format!(
            "списание по худшей цене {} больше свободного {cap}",
            c.spend_max.round_dp(4)
        )),
        Verdict::Reject { reason, .. } => Err(reason),
    }
}
