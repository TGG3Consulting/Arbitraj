//! Файл: core/final_fit.rs
//! Что делает: решение 294 (Тигран 15.11: потолок сделки — предел, а не обязательный объём) — финальный расчёт
//! (profit.rs final_calc) на найденном объёме не поместился, и исполнимый объём у исполнителя не получен с кодом
//! шлюза NoRoute (у DEX — путь через промежуточную монету, шаг которого не исполним целиком: такой путь, в отличие от
//! одного пула, неполной котировки не отдаёт). Задана size.final_fit_steps — наибольший помещающийся объём ищется
//! ТЕМ ЖЕ правилом и с теми же границами, что верхняя граница подбора объёма (size_bounds.rs upper — бисекция в целых
//! шагах количества между 0 и найденным объёмом; инвариант бюджета — size_memo.rs: точка со списанием по худшей цене
//! больше бюджета — «слишком большая»), не больше size.final_fit_steps шагов. Каждая точка — тот же полный финальный
//! расчёт (все затраты, минимумы и пределы площадок, финальная котировка); пороги прибыли проверяет profit.rs после,
//! как раньше. Не нашлось — отказ, как раньше (текст с припиской).
//! Кто вызывает: core/profit.rs (shrink — только подбор кандидата; пересчёт предпроверки precheck.fast_recalc
//! уменьшение не делает — при отказе он, как раньше, уходит в полный подбор), тесты (largest).
//! От чего зависит: costs.rs (evaluate), size_bounds.rs (upper), size_memo.rs (Memo), size_curve.rs (point),
//! gateway/quote_stage.rs (final_scope), calc_detail.rs (этап подробного замера).
//! Настройки: size.final_fit_steps (вся система; необязательная; читает profit.rs).

use async_trait::async_trait;
use rust_decimal::Decimal;

use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::size::{Curve, Fail, Point};
use super::size_memo::Memo;

/// Кривая финального расчёта: полный расчёт costs.rs внутри final_scope (котировка исполнителя, решение 95).
struct FinalCurve<'a> {
    core: &'a Core,
    cx: &'a DirCtx,
}

#[async_trait]
impl Curve for FinalCurve<'_> {
    type Data = Calc;

    /// Финальный расчёт на объём. Ошибки: Fail из costs::evaluate.
    async fn at(&mut self, qty: Decimal) -> Result<Point<Calc>, Fail> {
        let eval = super::costs::evaluate(self.core, self.cx, qty);
        let c = crate::gateway::quote_stage::final_scope(eval).await?;
        Ok(super::size_curve::point(qty, c))
    }
}

/// Что делает: наибольший помещающийся объём меньше `qty` (бисекция size_bounds::upper на [0, qty]).
/// Принимает: кривую, объём, который не поместился (его отказ), бюджет, шаг количества (> 0), предел шагов.
/// Возвращает: точку на найденном объёме (расчёт прошёл; списание ≤ бюджета). Ошибки: Stop — расчёт невозможен;
/// TooLarge / TooSmall — помещающегося объёма не нашлось (меньше минимумов площадок или шагов не хватило).
pub async fn largest<C: Curve>(
    curve: &mut C,
    qty: Decimal,
    over: Fail,
    budget: Decimal,
    step: Decimal,
    steps: u32,
) -> Result<Point<C::Data>, Fail> {
    if step <= Decimal::ZERO {
        return Err(Fail::Stop(format!("шаг количества {step} не больше нуля")));
    }
    let mut m = Memo::new(curve, step, budget, steps.saturating_add(1));
    let Some(n) = m.units(qty).filter(|n| *n > 1) else {
        return Err(over);
    };
    // Объём n уже посчитан и не поместился — в память, без повторного расчёта; бисекция идёт на [0, n].
    m.map.insert(n, Err(over));
    let (lo, _) = super::size_bounds::upper(&mut m, n, steps).await?;
    m.at(lo).await
}

/// Что делает: уменьшение объёма финального расчёта до помещающегося (profit.rs final_calc).
/// Принимает: ядро, контекст, объём, его отказ (текст), бюджет, шаг количества, size.final_fit_steps.
/// Возвращает: расчёт на найденном объёме. Ошибки: текст — исходный отказ с причиной, почему меньший не найден.
pub(super) async fn shrink(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    why: &str,
    budget: Decimal,
    step: Decimal,
    steps: u32,
) -> Result<Calc, String> {
    let mut curve = FinalCurve { core, cx };
    let over = Fail::TooLarge(why.to_string());
    let found = largest(&mut curve, qty, over, budget, step, steps);
    let name = || "уменьшение объёма до помещающегося (size.final_fit_steps)".into();
    match super::calc_detail::stage_detail(name, found).await {
        Ok(p) => Ok(p.data),
        Err(e) => Err(format!(
            "{why}; меньший помещающийся объём не найден за size.final_fit_steps = {steps}: {e}"
        )),
    }
}
