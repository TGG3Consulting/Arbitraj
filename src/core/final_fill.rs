//! Файл: core/final_fill.rs
//! Что делает: финальный расчёт (profit.rs final_calc) не поместился на найденном объёме — сколько исполнитель реально
//! исполнит на обеих сторонах (финальная котировка, меньшее из двух; решение 111 — пересчёт на исполнимый объём). Вынесено
//! из profit.rs без изменений (решение 294). Решение 294: исполнимый объём не получен с NoRoute «не помещается»
//! (признак too_large у ошибки шлюза; у DEX — путь через промежуточную, неполной котировки он не даёт), уменьшение
//! разрешено (fit: не внешний исполнитель — не exec.via_kyber) и задана size.final_fit_steps — наибольший
//! помещающийся объём (final_fit.rs shrink). Иначе — отказ прежним текстом «<отказ>; <площадка>: <причина>».
//! Будущее — в куче (fill_boxed): кадр расчёта кандидата не растёт (как precheck_fast.rs recalc_boxed).
//! Кто вызывает: core/profit.rs (final_calc).
//! От чего зависит: dex_account.rs, gateway/quote_stage.rs (final_scope), fit_volume.rs (QuoteErr), fit_settings.rs,
//! final_fit.rs, calc_detail.rs (этап подробного замера «исполнимый объём у исполнителя»).
//! Настройки: size.final_fit_steps (через fit_settings.rs).

use std::future::Future;
use std::pin::Pin;

use rust_decimal::Decimal;

use crate::core::types::Side;

use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::fit_volume::QuoteErr;

/// Итог: исполнимый объём (дальше — пересчёт на него в final_calc, решение 111) или готовый расчёт на уменьшенном
/// объёме (решение 294).
pub(super) enum Fill {
    Qty(Decimal),
    /// В коробке: расчёт большой, исполнимый объём — маленький (clippy large_enum_variant).
    Done(Box<Calc>),
}

/// Будущее исполнимого объёма в куче.
type Boxed<'a> = Pin<Box<dyn Future<Output = Result<Fill, String>> + Send + 'a>>;

/// Что делает: fill в куче. Принимает и ошибки: как fill.
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_boxed<'a>(
    core: &'a Core,
    cx: &'a DirCtx,
    qty: Decimal,
    why: &'a str,
    budget: Decimal,
    step: Decimal,
    fit: bool,
) -> Boxed<'a> {
    Box::pin(fill(core, cx, qty, why, budget, step, fit))
}

/// Что делает: исполнимый объём или (решение 294) уменьшение до помещающегося. Принимает: ядро, контекст, объём,
/// отказ финального расчёта (текст), бюджет, шаг количества, разрешено ли уменьшение. Возвращает: Fill.
/// Ошибки: текст «<отказ>; <причина>» — котировка не получена; уменьшение не нашло объёма — текст final_fit.rs.
async fn fill(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    why: &str,
    budget: Decimal,
    step: Decimal,
    fit: bool,
) -> Result<Fill, String> {
    let got = super::calc_detail::stage_detail(
        || "исполнимый объём у исполнителя".into(),
        final_fillable(core, cx, qty),
    )
    .await;
    match got {
        Ok(f) => Ok(Fill::Qty(f)),
        Err(e) if fit && e.too_large => {
            match super::fit_settings::tries(core, "size.final_fit_steps").await {
                Some(n) => super::final_fit::shrink(core, cx, qty, why, budget, step, n)
                    .await
                    .map(|c| Fill::Done(Box::new(c))),
                None => Err(format!("{why}; {}", e.text)),
            }
        }
        Err(e) => Err(format!("{why}; {}", e.text)),
    }
}

/// Сколько исполнитель реально исполнит на обеих сторонах (финальная котировка; меньшее из двух). Решение 111.
/// Ошибки: площадка не подключена, котировка не получена (решение 294: с признаком «не помещается» — QuoteErr).
async fn final_fillable(core: &Core, cx: &DirCtx, qty: Decimal) -> Result<Decimal, QuoteErr> {
    let d = &cx.dir;
    // Решение 132: DEX — кошелёк строки маппинга направления.
    let deps = &core.inner.deps;
    let a = super::dex_account::venue_dir(deps, d, &d.buy_on).await?;
    let b = super::dex_account::venue_dir(deps, d, &d.sell_on).await?;
    // Решение 211: каждой площадке — её тикер монеты (Presence::ticker в DirCtx).
    let (on_a, on_b) = (cx.on_a(), cx.on_b());
    let (qa, qb) = crate::gateway::quote_stage::final_scope(async {
        tokio::join!(
            a.quote(&on_a, Side::Buy, qty),
            b.quote(&on_b, Side::Sell, qty)
        )
    })
    .await;
    let qa = qa.map_err(|e| QuoteErr::of(&d.buy_on.0, &e))?;
    let qb = qb.map_err(|e| QuoteErr::of(&d.sell_on.0, &e))?;
    Ok(qa.fillable_qty.min(qb.fillable_qty))
}
