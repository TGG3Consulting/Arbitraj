//! Файл: core/size_curve.rs
//! Что делает: кривая «прибыль от объёма» ядра для подбора объёма (решение 66) — каждый объём считает полный
//! расчёт costs.rs (все затраты); параметры поиска: шаг количества пары биржи (stepSize; A, если A — биржа, иначе
//! B), изломы — накопленные объёмы уровней стакана A (asks) и B (bids), настройки size.*.
//! Решение 217: src — снимок рынка момента решения (расчёт «без потолка» по сделке, core/frozen.rs); решение 239 —
//! настройки size.* тогда из копии на момент решения (Frozen.decided.settings).
//! Решение 218: SourceCurve — та же кривая над любым источником данных (costs_source.rs; пересчёт старых записей «без
//! потолка» по окнам), search_params — параметры поиска из настроек size.* при известных шаге и изломах.
//! Кто вызывает: core/profit.rs, card_max.rs, uncapped.rs, uncapped_recompute.rs (SourceCurve, search_params).
//! От чего зависит: costs.rs, costs_source.rs, dirctx.rs, deps.rs, engine.rs (рынок), size.rs, settings.
//! Бюджет каждой точки — по наибольшему списанию Calc.spend_max (количество × худшая цена покупки); решение 246 —
//! у кривой боя (profit.rs) бюджет проверяется уже после стороны A (CoreCurve.budget), без котировок B.
//! Настройки: size.bisect_steps, size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals (общие; значений нет —
//! не задана хоть одна — подбора нет, сделки нет).

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::core::types::{OrderBook, PriceLevel};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::costs::{self, Calc, Source};
use super::dirctx::DirCtx;
use super::engine::{Core, rd};
use super::frozen::Frozen;
use super::size::{Curve, Fail, Params, Point};

/// Кривая по полному расчёту направления.
pub struct CoreCurve<'a> {
    pub core: &'a Core,
    pub cx: &'a DirCtx,
    /// Решение 217: снимок рынка момента решения (расчёт «без потолка» по сделке); None — живые котировки.
    pub src: Option<&'a Frozen>,
    /// Решение 246: бюджет подбора объёма (тот же, что Params.budget) — объём сверх него отбрасывается после стороны
    /// A, без котировок B (costs.rs evaluate_capped). None — полный расчёт каждого объёма: поиск без бюджета (size_max,
    /// size_uncapped — Memo::beyond) должен видеть точки сверх бюджета.
    pub budget: Option<Decimal>,
}

#[async_trait]
impl Curve for CoreCurve<'_> {
    type Data = Calc;

    /// Полный расчёт на объём (решение 246: с бюджетом — до стороны B, если списание больше бюджета).
    /// Ошибки: Fail из costs::evaluate_capped.
    async fn at(&mut self, qty: Decimal) -> Result<Point<Calc>, Fail> {
        let src = costs::Live {
            core: self.core,
            src: self.src,
        };
        let c = costs::evaluate_capped(self.cx, qty, &src, self.budget).await?;
        Ok(point(qty, c))
    }
}

/// Кривая по полному расчёту над источником данных (решение 218, costs_source.rs) — та же модель, что CoreCurve.
pub struct SourceCurve<'a, S> {
    pub cx: &'a DirCtx,
    pub src: &'a S,
}

#[async_trait]
impl<S: Source> Curve for SourceCurve<'_, S> {
    type Data = Calc;

    /// Полный расчёт на объём. Ошибки: Fail из costs::evaluate_with.
    async fn at(&mut self, qty: Decimal) -> Result<Point<Calc>, Fail> {
        let c = costs::evaluate_with(self.cx, qty, self.src).await?;
        Ok(point(qty, c))
    }
}

/// Точка кривой из расчёта. Бюджет — по наибольшему списанию (худшая цена покупки), не по средней (решение 66).
/// Ошибок нет.
fn point(qty: Decimal, c: Calc) -> Point<Calc> {
    Point {
        qty,
        invested: c.spend_max,
        arrive: c.arrive,
        profit: c.profit_usd,
        data: c,
    }
}

/// Параметры поиска. Принимает: ядро, контекст направления, бюджет, оценку цены покупки (> 0), снимок рынка (решение
/// 217: изломы — по стаканам снимка; None — живые стаканы ядра).
/// Ошибки: текст — не задана настройка size.*, шаг количества неизвестен (ни одна сторона не биржа).
pub fn params(
    core: &Core,
    cx: &DirCtx,
    budget: Decimal,
    price: Decimal,
    src: Option<&Frozen>,
) -> Result<Params, String> {
    let step = cx
        .pair_a
        .as_ref()
        .or(cx.pair_b.as_ref())
        .map(|p| p.step_size)
        .ok_or("шаг количества неизвестен: ни одна сторона сделки не биржа")?;
    let d = &cx.dir;
    // Решение 211: стаканы — по тикеру площадки (Presence::ticker стороны направления).
    let (ta, tb) = (&cx.a.ticker, &cx.b.ticker);
    let cum = |b: Option<&OrderBook>, buy: bool| -> Vec<Decimal> {
        b.map(|b| cumulative(if buy { &b.asks } else { &b.bids }))
            .unwrap_or_default()
    };
    let (buy_breaks, sell_breaks) = {
        let market = rd(&core.inner.market);
        match src {
            Some(f) => (cum(f.book(false), true), cum(f.book(true), false)),
            None => (
                cum(market.book(&d.buy_on, ta), true),
                cum(market.book(&d.sell_on, tb), false),
            ),
        }
    };
    // Решение 239: у снимка со входами момента решения — настройки на тот момент.
    let s = src
        .and_then(|f| f.decided.as_deref())
        .map_or(&core.inner.deps.settings, |x| &x.settings);
    search_params(s, step, budget, price, buy_breaks, sell_breaks)
}

/// Что делает: параметры поиска при известных шаге количества и изломах (решение 218: общий кусок params и пересчёта
/// по окнам). Принимает: настройки, шаг, бюджет, оценку цены покупки (> 0), изломы покупки и продажи.
/// Возвращает: Params. Ошибки: текст — не задана настройка size.*.
pub fn search_params(
    s: &Store,
    step: Decimal,
    budget: Decimal,
    price: Decimal,
    buy_breaks: Vec<Decimal>,
    sell_breaks: Vec<Decimal>,
) -> Result<Params, String> {
    let int = |name: &str| -> Result<u32, String> {
        let v = s.integer(name, At::default()).map_err(|e| e.message)?;
        u32::try_from(v).map_err(|_| format!("{name} = {v}: вне пределов"))
    };
    let tolerance_pct = s
        .decimal("size.tolerance_pct", At::default())
        .map_err(|e| e.message)?;
    Ok(Params {
        step,
        budget,
        guess: budget / price,
        buy_breaks,
        sell_breaks,
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
