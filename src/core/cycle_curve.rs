//! Файл: core/cycle_curve.rs
//! Что делает: кривая «прибыль от объёма» цикла DEX ↔ DEX (решение 124; техдолг «динамический размер цикла») для общего
//! поиска объёма ядра (size_search.rs, решение 66): объём = вход цикла в базовой монете, расчёт точки — шлюз
//! Venue::dex_cycle_quote (адаптер считает цикл по состоянию пулов в памяти, без узла). Вложение = вход × курс базовой
//! в USDT (Point.invested ≤ бюджета держит size_memo.rs), прибыль = чистая прибыль цикла в базовой × курс (все затраты
//! решения 85 — у адаптера). Приход на B не нужен (перевода нет) — arrive = вход.
//! Отказ адаптера: NoRoute (шаг не исполним целиком — объём велик) → TooLarge (меньший объём может подойти);
//! прочее → Stop (расчёт невозможен).
//! Решение 336: batch — пачка входов одновременно (size.parallel_points).
//! Решение 344 (ревью t159 Н7): breaks — изломы ПЕРВОГО пула цикла (вход пула = вход цикла: перевод точный; контракт
//! quote_breaks, продажа базовой). Изломы следующих пулов по входу цикла — через обратный пересчёт выходов предыдущих
//! шагов (точным выходом), не сделано (docs/TASKS.md).
//! Кто вызывает: core/cycle_plan.rs.
//! От чего зависит: core/size.rs (Curve, Point, Fail), gateway (Venue), core/types (DexCycle, CycleQuote).
//! Настройки: нет.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::join_all;
use rust_decimal::Decimal;

use crate::core::types::{
    Asset, CycleQuote, DexCycle, DexRoute, RouteHop, RouteLeg, RouteMode, Side,
};
use crate::gateway::contract::Venue;
use crate::gateway::errors::ErrorCode;

use super::size::{Curve, Fail, Point};

/// Кривая одного цикла.
pub struct CycleCurve {
    pub venue: Arc<dyn Venue>,
    pub cycle: DexCycle,
}

#[async_trait]
impl Curve for CycleCurve {
    type Data = CycleQuote;

    /// Расчёт на вход qty базовой. Ошибки: TooLarge (шаг не исполним целиком), Stop (прочее).
    async fn at(&mut self, qty: Decimal) -> Result<Point<CycleQuote>, Fail> {
        self.eval(qty).await
    }

    /// Решение 336: пачка входов одновременно (точки независимы; при параллельных точках адаптер считает цикл в потоке
    /// счёта — gateway/calc_share.rs par). Ошибки: у каждой — как at.
    async fn batch(&mut self, qtys: Vec<Decimal>) -> Vec<Result<Point<CycleQuote>, Fail>> {
        let me = &*self;
        join_all(qtys.into_iter().map(|q| me.eval(q))).await
    }

    /// Решение 344 (ревью t159 Н7): изломы первого пула цикла по входу (см. шапку) — объёмы «покупки» (вход цикла).
    /// Ошибок нет: сбой — пусто.
    async fn breaks(
        &mut self,
        _data: &CycleQuote,
        max_qty: Decimal,
        limit: u32,
    ) -> (Vec<Decimal>, Vec<Decimal>) {
        let Some(h) = self.cycle.hops.first() else {
            return (Vec::new(), Vec::new());
        };
        let route = DexRoute {
            mode: RouteMode::Direct,
            legs: vec![RouteLeg {
                share: Decimal::ONE,
                hops: vec![RouteHop {
                    family: h.family.clone(),
                    kind: h.kind,
                    pool: h.pool.clone(),
                }],
                expected_out: Decimal::ZERO,
            }],
            block: self.cycle.block,
            gas_units: 0,
            executable: true,
            exec_note: None,
        };
        let base = Asset {
            ticker: self.cycle.base_ticker.clone(),
            bsc_contract: Some(self.cycle.base),
        };
        let r = self
            .venue
            .quote_breaks(&base, Side::Sell, &route, max_qty, limit);
        (r.await.unwrap_or_default(), Vec::new())
    }
}

impl CycleCurve {
    /// Расчёт одного входа (общий для at и batch). Ошибки: как at.
    async fn eval(&self, qty: Decimal) -> Result<Point<CycleQuote>, Fail> {
        match self.venue.dex_cycle_quote(&self.cycle, qty, false).await {
            Ok(c) => Ok(Point {
                qty,
                invested: qty * c.base_usdt,
                arrive: qty,
                profit: c.net_base * c.base_usdt,
                data: c,
            }),
            Err(e) if e.code == ErrorCode::NoRoute => Err(Fail::TooLarge(e.message)),
            Err(e) => Err(Fail::Stop(e.message)),
        }
    }
}
