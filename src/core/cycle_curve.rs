//! Файл: core/cycle_curve.rs
//! Что делает: кривая «прибыль от объёма» цикла DEX ↔ DEX (решение 124; техдолг «динамический размер цикла») для общего
//! поиска объёма ядра (size_search.rs, решение 66): объём = вход цикла в базовой монете, расчёт точки — шлюз
//! Venue::dex_cycle_quote (адаптер считает цикл по состоянию пулов в памяти, без узла). Вложение = вход × курс базовой
//! в USDT (Point.invested ≤ бюджета держит size_memo.rs), прибыль = чистая прибыль цикла в базовой × курс (все затраты
//! решения 85 — у адаптера). Приход на B не нужен (перевода нет) — arrive = вход.
//! Отказ адаптера: NoRoute (шаг не исполним целиком — объём велик) → TooLarge (меньший объём может подойти);
//! прочее → Stop (расчёт невозможен).
//! Кто вызывает: core/cycle_plan.rs.
//! От чего зависит: core/size.rs (Curve, Point, Fail), gateway (Venue), core/types (DexCycle, CycleQuote).
//! Настройки: нет.

use std::sync::Arc;

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::core::types::{CycleQuote, DexCycle};
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
