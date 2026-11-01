//! Файл: core/profit.rs
//! Что делает: решение по кандидату (шаг 4 ТЗ, решение 66): сначала подбирает объём с наибольшей прибылью в долларах
//! после всех затрат при условии «вложение ≤ бюджета» (size_search.rs по кривой size_curve.rs — полный расчёт
//! costs.rs на каждый объём), и только потом проверяет пороги: прибыль ≥ money.min_profit_usd и ≥ money.min_profit_pct
//! от вложенного. Иначе — отказ с причиной и лучшим расчётом (для подбора порогов). В расчёт кладётся объяснение
//! выбора: «оптимум 7.3 WFI: прибыль 0.41; на бюджете 20 (9.75 WFI) — 0.33».
//! Кто вызывает: core/candidates.rs (очередь тяжёлого расчёта), core/precheck.rs (пересчёт перед покупкой).
//! От чего зависит: costs.rs, dirctx.rs, limits.rs, size_curve.rs, size_search.rs, market_state.rs, settings.
//! Решение 95: найденный оптимум пересчитывается ФИНАЛЬНОЙ котировкой (gateway/quote_stage.rs) — у DEX при
//! exec.via_kyber это цена KyberSwap (тот, кто исполнит); пороги проверяются по ней. Отказ финальной котировки —
//! отказ кандидата с причиной.
//! Настройки: money.min_profit_usd, money.min_profit_pct, size.* (+ всё, что читают costs.rs, dirctx.rs, limits.rs).

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{Direction, Side};
use crate::settings::read::At;

use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::{Core, rd};
use super::size::SizeChoice;
use super::size_curve::{CoreCurve, params};

/// Итог расчёта кандидата.
#[derive(Debug, Clone, Serialize)]
pub enum Verdict {
    /// Прибыль выше порога — сделка.
    Accept(Calc),
    /// Отказ: причина и лучший найденный расчёт (если был).
    Reject { reason: String, best: Option<Calc> },
}

/// Расчёт кандидата. Принимает: ядро, направление, режим (зафиксирован вызывающим: очередь кандидатов — на расчёт,
/// предпроверка сделки — всегда Live, решение 74). Возвращает: Verdict. Ошибок нет — всё в Reject.
pub async fn decide(core: &Core, d: &Direction, mode: &super::dry::Mode) -> Verdict {
    decide_capped(core, d, mode, None).await
}

/// То же, но бюджет не больше `cap` (решение 103: свободный баланс A прямо перед ордером меньше бюджета — объём
/// подбирается заново под свободный остаток, refit.rs). Принимает: ядро, направление, режим, предел. Ошибок нет.
pub async fn decide_capped(
    core: &Core,
    d: &Direction,
    mode: &super::dry::Mode,
    cap: Option<Decimal>,
) -> Verdict {
    let reject = |reason: String, best: Option<Calc>| Verdict::Reject { reason, best };
    // Сухой режим без бюджета — сигналов нет, считать нечего (решение 74).
    if let super::dry::Mode::Dry(Err(e)) = mode {
        return reject(e.clone(), None);
    }
    let cx = match DirCtx::build(core, d) {
        Ok(c) => c,
        Err(e) => return reject(e, None),
    };
    let s = &core.inner.deps.settings;
    let (min_usd, min_pct) = match (
        s.decimal("money.min_profit_usd", At::default()),
        s.decimal("money.min_profit_pct", At::default()),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return reject(e.message, None),
    };
    let budget = match super::limits::budget(core, &cx, mode).await {
        Ok(b) => cap.map_or(b, |c| b.min(c)),
        Err(e) => return reject(e, None),
    };
    let price = rd(&core.inner.market).best(&d.buy_on, &d.asset.ticker, Side::Buy);
    let price = match price.or(rd(&core.inner.spreads)
        .get(d)
        .and_then(|v| v.prices.as_ref().map(|p| p.buy)))
    {
        Some(p) if !p.is_zero() => p,
        _ => return reject("нет цены покупки на A".into(), None),
    };
    let p = match params(core, &cx, budget, price) {
        Ok(p) => p,
        Err(e) => return reject(e, None),
    };
    let mut curve = CoreCurve { core, cx: &cx };
    let found = match super::size_search::search(&mut curve, &p).await {
        Ok(f) => f,
        Err(e) => return reject(e.to_string(), None),
    };
    let mut choice = found.choice;
    choice.why = why(&choice, &d.asset.ticker);
    // Решение 95: объём найден дешёвыми котировками; оптимум пересчитывается ФИНАЛЬНОЙ котировкой (quote_stage) —
    // площадка отдаёт цену того, кто исполнит (у DEX при exec.via_kyber — KyberSwap). Вердикт — по ней.
    let mut c = match final_calc(core, &cx, choice.qty, budget, p.step).await {
        Ok(f) => f,
        Err(e) => {
            return reject(
                format!("финальная котировка: {e} ({})", choice.why),
                Some(found.best.data),
            );
        }
    };
    // Финальная цена хуже — объём мог быть ужат до бюджета (final_calc): объём сделки — фактический.
    if c.qty != choice.qty {
        choice.why = format!(
            "{}; по финальной котировке объём ужат до бюджета: {} {}",
            choice.why,
            c.qty.normalize(),
            d.asset.ticker
        );
        choice.qty = c.qty;
    }
    c.size = Some(choice.clone());
    if c.profit_usd < min_usd {
        return reject(
            format!(
                "прибыль {} USDT ниже порога {min_usd} ({})",
                c.profit_usd.round_dp(4),
                choice.why
            ),
            Some(c),
        );
    }
    if c.profit_pct < min_pct {
        return reject(
            format!(
                "прибыль {}% ниже порога {min_pct}% ({})",
                c.profit_pct.round_dp(4),
                choice.why
            ),
            Some(c),
        );
    }
    Verdict::Accept(c)
}

/// Финальный расчёт на найденном объёме (решение 95): тот же полный расчёт costs.rs, но внутри
/// gateway::quote_stage::final_scope — адаптер отдаёт котировку исполнителя. Списание по худшей цене не должно выйти за
/// бюджет: если финальная цена хуже и списание вышло за бюджет (объём подобран впритык к бюджету по своей цене) —
/// объём один раз ужимается пропорционально (qty × бюджет / списание, вниз до шага) и считается заново.
/// Принимает: ядро, контекст, объём, бюджет, шаг количества. Возвращает: Calc. Ошибки: текст (отказ расчёта, бюджет).
async fn final_calc(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    budget: Decimal,
    step: Decimal,
) -> Result<Calc, String> {
    let eval =
        |q: Decimal| crate::gateway::quote_stage::final_scope(super::costs::evaluate(core, cx, q));
    let c = eval(qty).await.map_err(|f| f.to_string())?;
    if c.spend_max <= budget {
        return Ok(c);
    }
    let over = |c: &Calc| {
        format!(
            "списание по худшей цене {} больше бюджета {budget}",
            c.spend_max.round_dp(4)
        )
    };
    if step <= Decimal::ZERO || c.spend_max <= Decimal::ZERO {
        return Err(over(&c));
    }
    let q = (qty * budget / c.spend_max / step).floor() * step;
    if q <= Decimal::ZERO || q >= qty {
        return Err(over(&c));
    }
    let c2 = eval(q).await.map_err(|f| f.to_string())?;
    if c2.spend_max > budget {
        return Err(over(&c2));
    }
    Ok(c2)
}

/// Почему объём лучший — текст для журнала и «Кандидатов». Принимает: итог подбора, тикер. Ошибок нет.
pub fn why(c: &SizeChoice, ticker: &str) -> String {
    let r = |v: Decimal| v.round_dp(4).normalize();
    let head = format!(
        "оптимум {} {ticker}: прибыль {}",
        c.qty.normalize(),
        r(c.profit)
    );
    let budget = if c.qty == c.max_qty {
        format!(
            "это и есть наибольший объём в пределах бюджета {}",
            r(c.budget)
        )
    } else {
        format!(
            "на бюджете {} ({} {ticker}) — {}",
            r(c.budget),
            c.max_qty.normalize(),
            r(c.at_budget_profit)
        )
    };
    let mut exact = String::new();
    if !c.max_exact {
        exact.push_str("; граница бюджета не уточнена до шага (мало size.bisect_steps)");
    }
    if c.capped {
        exact.push_str("; поиск остановлен пределом size.max_evals — лучший из посчитанных");
    }
    format!(
        "{head}; {budget}; допустимо {}–{} {ticker}, посчитано объёмов {}, пиков {}{exact}",
        c.min_qty.normalize(),
        c.max_qty.normalize(),
        c.evals,
        c.peaks
    )
}
