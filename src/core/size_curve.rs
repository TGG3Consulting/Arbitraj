//! Файл: core/size_curve.rs
//! Что делает: кривая «прибыль от объёма» ядра для подбора объёма (решение 66) — каждый объём считает полный
//! расчёт costs.rs (все затраты); параметры поиска: шаг количества пары биржи (stepSize; A, если A — биржа, иначе
//! B), изломы — накопленные объёмы уровней стакана A (asks) и B (bids), настройки size.*.
//! Решение 217: src — снимок рынка момента решения (расчёт «без потолка» по сделке, core/frozen.rs); решение 239 —
//! настройки size.* тогда из копии на момент решения (Frozen.decided.settings).
//! Решение 218: SourceCurve — та же кривая над любым источником данных (costs_source.rs; пересчёт старых записей «без
//! потолка» по окнам), search_params — параметры поиска из настроек size.* при известных шаге и изломах.
//! Параметры поиска (params, search_params, cumulative) — core/size_params.rs (ревью t159: вынесено), пути прежние.
//! Кто вызывает: core/profit.rs, card_max.rs, uncapped.rs, uncapped_recompute.rs (SourceCurve, search_params).
//! От чего зависит: costs.rs, costs_source.rs, dirctx.rs, deps.rs, engine.rs (рынок), size.rs, settings.
//! Бюджет каждой точки — по наибольшему списанию Calc.spend_max (количество × худшая цена покупки); решение 246 —
//! у кривой боя (profit.rs) бюджет проверяется уже после стороны A (CoreCurve.budget), без котировок B.
//! Решение 336: batch — пачка точек одновременно (join_all на одной задаче: котировки DEX каждой точки уходят в свои
//! потоки счёта адаптера, ожидание — общее); size.parallel_points и size.bisect_probes — в Params (size_par.rs).
//! Задача t159 (ревью t159): Curve::pct и pct_base — процент прибыли расчёта и его база Calc.invested (решение 342);
//! CoreCurve::breaks — изломы пулов DEX-сторон маршрута точки (решение 344, size_breaks.rs); search_params читает
//! size.scan_above_pct (size_extra.rs); params — участки допустимости продажи по стакану B (решение 343, size_scan.rs
//! regions, в логике), признаки «бюджет монотонен» (A — биржа) и «есть сторона DEX» (Б3).
//! Настройки: size.bisect_steps, size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals (общие; значений нет —
//! не задана хоть одна — подбора нет, сделки нет); size.parallel_points, size.bisect_probes (необязательные, решение 336);
//! dex.quote_reuse [площадка направления] — признак короба с повторным использованием (решение 336, один раз на подбор).

use async_trait::async_trait;
use futures_util::future::join_all;
use rust_decimal::Decimal;

use super::costs::{self, Calc, Source};
use super::dirctx::DirCtx;
use super::engine::Core;
use super::frozen::Frozen;
use super::size::{Curve, Fail, Point};

/// Параметры поиска вынесены в size_params.rs (ревью t159: файл перерос 200 строк) — пути прежние.
pub use super::size_params::{cumulative, params, search_params};

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
        self.eval(qty).await
    }

    /// Решение 336: пачка точек одновременно (точки независимы). Ошибки: у каждой — как at.
    async fn batch(&mut self, qtys: Vec<Decimal>) -> Vec<Result<Point<Calc>, Fail>> {
        let me = &*self;
        join_all(qtys.into_iter().map(|q| me.eval(q))).await
    }

    /// Решение 342: процент прибыли расчёта от вложенного (Calc.profit_pct, costs.rs). Ошибок нет.
    fn pct(data: &Calc) -> Option<Decimal> {
        Some(data.profit_pct)
    }

    /// Ревью t159 Б1: база процента — Calc.invested (объём × средняя цена покупки, costs.rs). Ошибок нет.
    fn pct_base(data: &Calc) -> Option<Decimal> {
        Some(data.invested)
    }

    /// Решение 344: изломы пулов DEX-сторон маршрута точки (size_breaks.rs core_breaks); у снимка рынка — нет (живые
    /// пулы другого момента). Ошибок нет.
    async fn breaks(
        &mut self,
        data: &Calc,
        max_qty: Decimal,
        limit: u32,
    ) -> (Vec<Decimal>, Vec<Decimal>) {
        if self.src.is_some() {
            return (Vec::new(), Vec::new());
        }
        super::size_breaks::core_breaks(self.core, self.cx, data, max_qty, limit).await
    }
}

impl CoreCurve<'_> {
    /// Расчёт одной точки без изменения кривой (общий для at и batch). Ошибки: Fail из costs::evaluate_capped.
    async fn eval(&self, qty: Decimal) -> Result<Point<Calc>, Fail> {
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
        self.eval(qty).await
    }

    /// Решение 336: пачка точек одновременно (точки независимы). Ошибки: у каждой — как at.
    async fn batch(&mut self, qtys: Vec<Decimal>) -> Vec<Result<Point<Calc>, Fail>> {
        let me = &*self;
        join_all(qtys.into_iter().map(|q| me.eval(q))).await
    }

    /// Решение 342: процент прибыли расчёта от вложенного (Calc.profit_pct, costs.rs). Ошибок нет.
    fn pct(data: &Calc) -> Option<Decimal> {
        Some(data.profit_pct)
    }

    /// Ревью t159 Б1: база процента — Calc.invested. Ошибок нет.
    fn pct_base(data: &Calc) -> Option<Decimal> {
        Some(data.invested)
    }
}

impl<S: Source> SourceCurve<'_, S> {
    /// Расчёт одной точки (общий для at и batch). Ошибки: Fail из costs::evaluate_with.
    async fn eval(&self, qty: Decimal) -> Result<Point<Calc>, Fail> {
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
