//! Файл: core/size_retry.rs
//! Что делает: ревью t159 Н2 (решение 342) — подбор с порогом % как ограничением выбирает объём у самой границы
//! «% ≥ порога» (size_pct.rs, бисекция до ширины остановки); финальная котировка (решение 95: свежее состояние, у DEX
//! при exec.via_kyber — исполнитель) может дать чуть хуже, и % окажется ниже порога. Тогда — ОДИН пересчёт финальной
//! котировкой на ближайшем меньшем посчитанном объёме, проходящем оба порога (Found.fallback): прошёл оба порога —
//! сделка на нём, иначе отказ как раньше. Без константы: объём — из посчитанных точек подбора.
//! Кто вызывает: core/profit.rs.
//! От чего зависит: core/profit.rs (final_calc), core/calc_timing.rs, core/size.rs (SizeChoice).
//! Настройки: нет (пороги — money.min_profit_usd / money.min_profit_pct направления, через profit.rs).

use rust_decimal::Decimal;

use super::calc_timing::stage;
use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::size::SizeChoice;

/// Что делает: пересчёт финальной котировкой на запасном объёме. Принимает: ядро, контекст направления, (объём,
/// прибыль подбора) запасного, бюджет, шаг количества, пороги (USDT, %), выбор подбора. Возвращает: расчёт, прошедший
/// оба порога, с пояснением выбора; None — финальная не получилась или не прошла порог. Ошибок нет.
pub async fn retry(
    core: &Core,
    cx: &DirCtx,
    fb: (Decimal, Decimal),
    budget: Decimal,
    step: Decimal,
    min: (Decimal, Decimal),
    choice: &SizeChoice,
) -> Option<Calc> {
    let fin = super::profit::final_calc(core, cx, fb.0, budget, step);
    let mut c = stage(
        || "финальная котировка (меньший проходящий объём)".into(),
        fin,
    )
    .await
    .ok()?;
    if c.profit_usd < min.0 || c.profit_pct < min.1 {
        return None;
    }
    let t = &cx.dir.asset.ticker;
    let mut ch = choice.clone();
    ch.why = format!(
        "{}; по финальной котировке на {} {t} прибыль в % ниже порога {} — взят ближайший меньший проходящий оба \
         порога объём {} {t}",
        choice.why,
        choice.qty.normalize(),
        min.1,
        c.qty.normalize()
    );
    (ch.qty, ch.profit) = (c.qty, fb.1);
    c.size = Some(ch);
    Some(c)
}
