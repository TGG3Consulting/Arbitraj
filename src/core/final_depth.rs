//! Числовой отказ конкретной ноги финального расчёта. Данные живут только внутри
//! одной попытки. Они задают следующий объём для проверки свежими котировками.
//! Вызывают profit, costs, costs_exit и executor. Использует обычную модель costs и часы предпроверки.

use rust_decimal::Decimal;
use std::time::Instant;

use super::costs::{Calc, Live};
use super::dirctx::DirCtx;
use super::engine::Core;
use super::size::Fail;
use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

tokio::task_local! {
    /// Абсолютный срок единственной дополнительной попытки сигнала до первой заявки.
    static BEFORE_BUY: std::cell::Cell<Option<Instant>>;
}

/// Общий срок сигнала, пока дополнительный расчёт не начался.
pub(super) fn before_buy() -> Option<Instant> {
    BEFORE_BUY.try_with(std::cell::Cell::get).ok().flatten()
}

/// Запускает этап до покупки и возвращает остаток права на попытку для следующего этапа.
pub(super) async fn before_order<F: std::future::Future>(
    deadline: Option<Instant>,
    task: F,
) -> (F::Output, Option<Instant>) {
    BEFORE_BUY
        .scope(std::cell::Cell::new(deadline), async {
            (task.await, before_buy())
        })
        .await
}

/// Мягкий срок поиска может сократить оставшееся время на этой стадии.
fn reserve(until: Instant) -> Option<Instant> {
    BEFORE_BUY
        .try_with(|slot| {
            let end = slot.get()?.min(until);
            if Instant::now() >= end {
                return None;
            }
            slot.set(None);
            Some(end)
        })
        .ok()
        .flatten()
}

#[derive(Clone, Copy)]
pub(super) enum Leg {
    Buy,
    Sell { arrive: Decimal },
    Reserve { arrive: Decimal },
}

#[derive(Default)]
pub(super) struct Observation {
    buy: Decimal,
    gap: Option<(Leg, Decimal, Decimal)>,
}

impl Observation {
    /// Удалить предыдущий замер перед повтором штатного пересчёта.
    pub fn clear(&mut self) {
        self.gap = None;
    }

    /// Кандидат в единицах покупки. Фиксированную комиссию вывода учитываем отдельно,
    /// остальной выход оцениваем по измеренному приходу до округления на B. При
    /// нелинейном тарифе это только приближение, требуется свежий полный расчёт.
    fn candidate(
        &self,
        h: Decimal,
        fixed: Decimal,
        step_a: Decimal,
        step_b: Decimal,
    ) -> Option<Decimal> {
        let (leg, requested, filled) = self.gap?;
        if filled <= Decimal::ZERO || filled >= requested || step_a <= Decimal::ZERO {
            return None;
        }
        let q = match leg {
            Leg::Buy => filled.checked_div(step_a)?.floor().checked_mul(step_a)?,
            Leg::Reserve { arrive } | Leg::Sell { arrive } => {
                if arrive <= Decimal::ZERO
                    || step_b <= Decimal::ZERO
                    || fixed < Decimal::ZERO
                    || self.buy <= fixed
                {
                    return None;
                }
                let available = match leg {
                    Leg::Reserve { .. } => filled.checked_mul(h)?,
                    _ => filled,
                };
                let target = available.checked_div(step_b)?.floor().checked_mul(step_b)?;
                if target <= Decimal::ZERO {
                    return None;
                }
                let gross = target
                    .checked_mul(self.buy - fixed)?
                    .checked_div(arrive)?
                    .checked_add(fixed)?;
                gross.checked_div(step_a)?.ceil().checked_mul(step_a)?
            }
        };
        (q > Decimal::ZERO && q < self.buy).then_some(q)
    }
}

/// Применяет штатную проверку глубины. Сохраняет фактическое измерение только при её
/// числовом отказе. Ошибка самого шлюза до этой функции не доходит.
pub(super) fn check(
    venue: &str,
    leg: Leg,
    requested: Decimal,
    filled: Decimal,
    observation: Option<&mut Observation>,
) -> Result<(), Fail> {
    super::costs_quote::enough_qty(venue, filled, requested).inspect_err(|_| {
        if let Some(o) = observation {
            o.gap = Some((leg, requested, filled));
        }
    })
}

/// При следующем штатном расчёте старый числовой отказ утрачивает силу, в том числе
/// если на новом объёме шлюз вернёт NoRoute, старые данные или неизвестный исход.
pub(super) async fn evaluate(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    mut capture: Option<&mut Observation>,
) -> Result<Calc, Fail> {
    if let Some(o) = capture.as_deref_mut() {
        o.buy = qty;
        o.clear();
    }
    let src = Live { core, src: None };
    crate::gateway::quote_stage::final_scope(super::costs::evaluate_observed(
        cx, qty, &src, None, capture,
    ))
    .await
}

fn cex_pair(kinds: &[(VenueId, VenueKind)], sides: [&VenueId; 2]) -> bool {
    sides.into_iter().all(|v| {
        kinds.iter().find(|(id, _)| id == v).map(|(_, k)| *k) == Some(VenueKind::Cex)
    })
}

/// Сначала выполняется весь обычный финальный путь. При исчерпанных лимитах,
/// внешнем исполнении или в предпроверке без заданного общего срока отказ сохраняется.
pub(super) async fn after_search(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    budget: Decimal,
    step: Decimal,
    min: (Decimal, Decimal),
    window: Option<Instant>,
) -> Result<(Calc, bool), String> {
    let mut observation = Observation::default();
    let capture = window.map(|_| &mut observation);
    let reason = match super::profit::final_standard(core, cx, qty, budget, step, capture).await {
        Ok(c) => return Ok((c, false)),
        Err(e) => e,
    };
    if let Some(deadline) = window
        && observation.gap.is_some()
        && {
            let kinds = super::engine::rd(&core.inner.kinds);
            cex_pair(&kinds, [&cx.dir.buy_on, &cx.dir.sell_on])
        }
        && let Some(c) = recover(core, cx, &observation, deadline, budget, step, min).await
    {
        return Ok((c, true));
    }
    Err(reason)
}

/// Ровно один дополнительный полный расчёт, только до общего срока, только на
/// биржевой паре. Этот путь не получает доступ к внешней финальной котировке DEX.
pub(super) async fn recover(
    core: &Core,
    cx: &DirCtx,
    observation: &Observation,
    deadline: Instant,
    budget: Decimal,
    step: Decimal,
    min: (Decimal, Decimal),
) -> Option<Calc> {
    if Instant::now() >= deadline {
        return None;
    }
    let fixed = cx.a.info.as_ref()?.withdraw_fee?;
    let q = observation.candidate(cx.h, fixed, step, cx.pair_b.as_ref()?.step_size)?;
    let deadline = reserve(deadline)?;
    let work = evaluate(core, cx, q, None);
    let c = tokio::time::timeout_at(deadline.into(), work)
        .await
        .ok()?
        .ok()?;
    (Instant::now() < deadline
        && c.spend_max <= budget
        && c.profit_usd >= min.0
        && c.profit_pct >= min.1)
        .then_some(c)
}

#[cfg(test)]
#[path = "../../tests/volume_cases/final_depth.rs"]
mod tests;
