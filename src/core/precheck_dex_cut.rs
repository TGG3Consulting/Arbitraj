//! Файл: core/precheck_dex_cut.rs
//! Что делает: решение 380 (задача t166) — объём сделки под то, что можно проверить симуляцией продажи до покупки.
//! Живой бот 17.11 (06:20–09:52 UTC): 350 отказов «продажа на bsc_dex не пройдёт: симуляция продажи до покупки не
//! прошла: у кошелька 0 единиц входа, нужно N; пула-источника … нет; запасной источник … не взят: у держателя … M
//! единиц, нужно N» (AVA bitget 242, FLOKI mexc 71, NXT mexc 32, YCT gate 3, MIRA bingx 2). Симуляция при этом не
//! запускалась вовсе: до покупки у кошелька монет нет, а источника, от которого их можно перевести внутри симуляции, на
//! весь объём не нашлось — и отказ шёл, хотя продать, возможно, можно.
//! Теперь адаптер отдаёт PresellOutcome::Short с объёмом F, который можно проверить (баланс кошелька + всё, что
//! дойдёт от всех источников вместе после налога), и здесь план пересчитывается тем же подбором объёма
//! (profit::decide_capped: все затраты, финальная котировка, пороги money.min_profit_usd / money.min_profit_pct) с
//! пределом «объём продажи ≤ F» (DirCtx.sell_max → costs.rs: больший объём — Fail::TooLarge, меньший может подойти;
//! признак монотонен: продажа растёт с покупкой) и списанием не больше списания исходного плана. Новый план ядро
//! симулирует заново на ТОЧНОМ объёме (precheck_dex_sell.rs) — сделка идёт только на объёме, проверенном целиком.
//! Почему не «проверить меньшую сумму и продать полную» (экстраполяция): симуляция на меньшей сумме доказывает, что
//! токен продаётся этим путём этим кошельком и налог на ЭТОЙ сумме; не доказывает лимит размера перевода/продажи у
//! токена (max tx, anti-whale), налог, зависящий от суммы, и прочую логику transfer токена — котировка пула и квотер
//! transfer токена не исполняют. Это была бы торговля вслепую; урезание объёма — нет.
//! Кто вызывает: core/precheck_dex_sell.rs (check), core/costs.rs (sell_max_ok).
//! От чего зависит: profit.rs (decide_capped), dirctx.rs (DirCtx.sell_max), dry.rs (Mode::Live), size.rs (Fail),
//! gateway/quote_purpose.rs (Purpose::Precheck), core::types::PresellShort.
//! Настройки: нет своих (всё, что читает profit.rs).

use rust_decimal::Decimal;

use crate::core::types::{Direction, PresellShort};

use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::profit::{self, Verdict};
use super::size::Fail;

/// Предел объёма продажи (решение 380) для расчёта одного объёма: продажа больше проверяемого — Fail::TooLarge
/// (меньший объём может подойти). Принимает: контекст направления (sell_max; None — предела нет), объём продажи.
/// Возвращает: Ok или TooLarge. Ошибок иных нет.
pub fn sell_max_ok(cx: &DirCtx, sell_qty: Decimal) -> Result<(), Fail> {
    match cx.sell_max {
        Some(m) if sell_qty > m => Err(Fail::TooLarge(format!(
            "продажа {} больше проверяемой симуляцией {} (решение 380)",
            sell_qty.normalize(),
            m.normalize()
        ))),
        _ => Ok(()),
    }
}

/// Новый план с объёмом продажи не больше проверяемого. Принимает: ядро, направление, исходный план (его списание —
/// предел), что сказал адаптер. Возвращает: расчёт (выше порогов, продажа ≤ short.fundable, списание ≤ исходного).
/// Ошибки: текст — контекст направления не собран, отказ подбора (в т. ч. ниже порогов на меньшем объёме).
pub async fn replan(
    core: &Core,
    d: &Direction,
    plan: &Calc,
    short: &PresellShort,
) -> Result<Calc, String> {
    let mut cx = DirCtx::build(core, d)?;
    cx.sell_max = Some(short.fundable);
    // Решение 348: назначение «предпроверка»; решение 345 — без мягкого срока; быстрого пути нет (объём другой).
    let work = profit::decide_capped(
        core,
        d,
        &super::dry::Mode::Live,
        Some(plan.spend_max),
        None,
        Some(Box::new(cx)),
        None,
    );
    let purpose = crate::gateway::quote_purpose::Purpose::Precheck;
    match crate::gateway::quote_purpose::scope(purpose, work).await {
        Verdict::Accept(c) if c.sell_qty <= short.fundable => Ok(c),
        Verdict::Accept(c) => Err(format!(
            "пересчёт на проверяемый объём: продажа {} больше проверяемой {}",
            c.sell_qty.normalize(),
            short.fundable.normalize()
        )),
        Verdict::Reject { reason, .. } => Err(format!(
            "пересчёт на проверяемый объём {} монет: {reason}",
            short.fundable.normalize()
        )),
    }
}

/// Строка журнала core.precheck: объём урезан до проверенного. Принимает: исходный план, что сказал адаптер, новый
/// объём продажи. Ошибок нет.
pub fn note(plan: &Calc, short: &PresellShort, sell_qty: Decimal) -> String {
    format!(
        "объём урезан до проверенного симуляцией (решение 380): продажа {} монет вместо {} (проверяемо не больше {}), \
         прибыль исходного плана {} USDT; причина: {}",
        sell_qty.normalize(),
        plan.sell_qty.normalize(),
        short.fundable.normalize(),
        plan.profit_usd.round_dp(4),
        short.why
    )
}
