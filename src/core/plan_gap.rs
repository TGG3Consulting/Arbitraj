//! Файл: core/plan_gap.rs
//! Что делает: решение 239 — кривая расчёта «без потолка», которая запоминает объёмы, недопустимые из-за валюты
//! затрат не из плана сделки (plan_costs.rs rates: на большом объёме появилась затрата в валюте, которой у плана не
//! было, например промежуточная монета маршрута DEX, — курса на момент входа нет). Такая точка — «объём недопустим»
//! (Fail::TooLarge, поиск идёт дальше), а не остановка всего расчёта; сколько таких объёмов и первая причина
//! пишутся в запись (Record.plan_gap).
//! Кто вызывает: core/uncapped.rs (обёртка кривой ядра).
//! От чего зависит: size.rs (Curve, Fail), plan_costs.rs (NOT_IN_PLAN).
//! Настройки: нет.

use async_trait::async_trait;
use rust_decimal::Decimal;

use super::plan_costs::NOT_IN_PLAN;
use super::size::{Curve, Fail, Point};

/// Кривая-обёртка: считает и запоминает недопустимые объёмы «валюты нет в плане».
pub struct PlanGap<C> {
    pub inner: C,
    /// Сколько объёмов недопустимо и первая причина с объёмом.
    count: u32,
    first: Option<String>,
}

impl<C> PlanGap<C> {
    /// Обёртка кривой. Принимает: кривую. Ошибок нет.
    pub fn new(inner: C) -> PlanGap<C> {
        PlanGap {
            inner,
            count: 0,
            first: None,
        }
    }

    /// Текст для записи: None — таких объёмов не было. Ошибок нет.
    pub fn note(&self) -> Option<String> {
        self.first.as_ref().map(|f| {
            format!(
                "объёмов недопустимо (валюта затрат не из плана сделки): {}; первый — {f}",
                self.count
            )
        })
    }
}

#[async_trait]
impl<C: Curve> Curve for PlanGap<C> {
    type Data = C::Data;

    /// Расчёт обёрнутой кривой; «валюты нет в плане» — запоминается. Ошибки: как у обёрнутой кривой.
    async fn at(&mut self, qty: Decimal) -> Result<Point<C::Data>, Fail> {
        let r = self.inner.at(qty).await;
        if let Err(Fail::TooLarge(t)) = &r
            && t.contains(NOT_IN_PLAN)
        {
            self.count += 1;
            self.first
                .get_or_insert_with(|| format!("объём {qty}: {t}"));
        }
        r
    }
}
