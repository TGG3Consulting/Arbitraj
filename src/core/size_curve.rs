//! Файл: core/size_curve.rs
//! Что делает: кривая «прибыль от объёма» ядра для подбора объёма (решение 66) — каждый объём считает полный
//! расчёт costs.rs (все затраты); параметры поиска: шаг количества пары биржи (stepSize; A, если A — биржа, иначе
//! B), изломы — накопленные объёмы уровней стакана A (asks) и B (bids), настройки size.*.
//! Кто вызывает: core/profit.rs.
//! От чего зависит: costs.rs, dirctx.rs, deps.rs, engine.rs (рынок), size.rs, settings.
//! Бюджет каждой точки — по наибольшему списанию Calc.spend_max (количество × худшая цена покупки).
//! Настройки: size.bisect_steps, size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals (общие; значений нет —
//! не задана хоть одна — подбора нет, сделки нет).

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::core::types::{OrderBook, PriceLevel};
use crate::settings::read::At;

use super::costs::{self, Calc};
use super::dirctx::DirCtx;
use super::engine::{Core, rd};
use super::size::{Curve, Fail, Params, Point};

/// Кривая по полному расчёту направления.
pub struct CoreCurve<'a> {
    pub core: &'a Core,
    pub cx: &'a DirCtx,
}

#[async_trait]
impl Curve for CoreCurve<'_> {
    type Data = Calc;

    /// Полный расчёт на объём. Ошибки: Fail из costs::evaluate.
    async fn at(&mut self, qty: Decimal) -> Result<Point<Calc>, Fail> {
        let c = costs::evaluate(self.core, self.cx, qty).await?;
        Ok(Point {
            qty,
            // Бюджет — по наибольшему списанию (худшая цена покупки), не по средней (решение 66).
            invested: c.spend_max,
            arrive: c.arrive,
            profit: c.profit_usd,
            data: c,
        })
    }
}

/// Параметры поиска. Принимает: ядро, контекст направления, бюджет, оценку цены покупки (> 0).
/// Ошибки: текст — не задана настройка size.*, шаг количества неизвестен (ни одна сторона не биржа).
pub fn params(core: &Core, cx: &DirCtx, budget: Decimal, price: Decimal) -> Result<Params, String> {
    let s = &core.inner.deps.settings;
    let int = |name: &str| -> Result<u32, String> {
        let v = s.integer(name, At::default()).map_err(|e| e.message)?;
        u32::try_from(v).map_err(|_| format!("{name} = {v}: вне пределов"))
    };
    let tolerance_pct = s
        .decimal("size.tolerance_pct", At::default())
        .map_err(|e| e.message)?;
    let step = cx
        .pair_a
        .as_ref()
        .or(cx.pair_b.as_ref())
        .map(|p| p.step_size)
        .ok_or("шаг количества неизвестен: ни одна сторона сделки не биржа")?;
    let d = &cx.dir;
    let market = rd(&core.inner.market);
    let cum = |b: Option<&OrderBook>, buy: bool| -> Vec<Decimal> {
        b.map(|b| cumulative(if buy { &b.asks } else { &b.bids }))
            .unwrap_or_default()
    };
    Ok(Params {
        step,
        budget,
        guess: budget / price,
        buy_breaks: cum(market.book(&d.buy_on, &d.asset.ticker), true),
        sell_breaks: cum(market.book(&d.sell_on, &d.asset.ticker), false),
        bisect_steps: int("size.bisect_steps")?,
        grid_points: int("size.grid_points")?,
        refine_steps: int("size.refine_steps")?,
        max_evals: int("size.max_evals")?,
        tolerance_pct,
    })
}

/// Накопленные объёмы уровней от лучшего (изломы средней цены). Ошибок нет.
pub fn cumulative(levels: &[PriceLevel]) -> Vec<Decimal> {
    let mut cum = Decimal::ZERO;
    levels
        .iter()
        .map(|l| {
            cum += l.qty;
            cum
        })
        .collect()
}
