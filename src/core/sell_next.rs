//! Файл: core/sell_next.rs
//! Что делает: решение 389 (задача t175) — продолжения продажи сделки, собранные в коробку для цикла sell_loop.rs:
//! вход в режим застрявшей монеты, повтор неушедшей продажи, продажа здесь по порогу после исчерпания повторов, повтор,
//! остаток и газ BNB продажи кнопкой. Каждое продолжение — тот же вызов, что раньше стоял в sell.rs /
//! sell_range_run.rs вложенно (те же аргументы, та же обёртка risk_count::auto там, где она была), но владеет всем,
//! что ему нужно (ядро — клоном, ошибка и причина — копией), — его можно исполнить после возврата из sell.
//! Функции не-async: будущее собирается сразу в куче, вне кадра вызывающего (как sell_range_run.rs stuck), и проверка
//! Send идёт вне тела рекурсивных async-функций.
//! Кто вызывает: core/sell.rs, core/sell_range_run.rs (через sell_loop::hop).
//! От чего зависит: sell_loop.rs (Job), stuck.rs (handle), sell_unsent.rs (retry, here), manual_sell.rs (retry,
//! remainder, gas), risk_count.rs (auto), stuck_reason.rs, op_error.rs, deal.rs, engine.rs (Core).
//! Настройки: нет.

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::sell_loop::Job;
use super::stuck_reason::StuckReason;

/// Режим застрявшей монеты (stuck::handle). Принимает: ядро, сделку, причину, auto — обернуть в risk_count::auto
/// (остаток после частичной продажи, газ BNB — решение 177; вход из sell_or_stuck и ветки 268 — без обёртки, как было).
/// Возвращает: продолжение. Ошибок нет.
#[inline(never)]
pub fn stuck(core: &Core, deal: Deal, why: StuckReason, auto: bool) -> Job {
    let c = core.clone();
    if auto {
        Box::pin(async move { super::risk_count::auto(super::stuck::handle(&c, deal, why)).await })
    } else {
        Box::pin(async move { super::stuck::handle(&c, deal, why).await })
    }
}

/// Повтор неушедшей продажи через режим застрявшей монеты (sell_unsent::retry под risk_count::auto). Принимает: ядро,
/// сделку, ошибку продажи, причину продажи, паузу (мс). Возвращает: продолжение. Ошибок нет.
#[inline(never)]
pub fn unsent_retry(core: &Core, deal: Deal, e: &OpError, why: &str, pause_ms: u64) -> Job {
    let (c, e, why) = (core.clone(), e.clone(), why.to_string());
    Box::pin(async move {
        let job = super::sell_unsent::retry(&c, deal, &e, &why, pause_ms);
        super::risk_count::auto(job).await
    })
}

/// Повторы исчерпаны — продажа здесь по порогу (sell_unsent::here под risk_count::auto, решение 229). Принимает: ядро,
/// сделку, ошибку продажи, предел повторов. Возвращает: продолжение. Ошибок нет.
#[inline(never)]
pub fn unsent_here(core: &Core, deal: Deal, e: &OpError, max: u32) -> Job {
    let (c, e) = (core.clone(), e.clone());
    Box::pin(
        async move { super::risk_count::auto(super::sell_unsent::here(&c, deal, &e, max)).await },
    )
}

/// Повтор продажи кнопкой (manual_sell::retry, решение 235; признак задачи — как был). Принимает: ядро, сделку, ошибку,
/// паузу (мс). Возвращает: продолжение. Ошибок нет.
#[inline(never)]
pub fn button_retry(core: &Core, deal: Deal, e: &OpError, pause_ms: u64) -> Job {
    let (c, e) = (core.clone(), e.clone());
    Box::pin(async move { super::manual_sell::retry(&c, deal, &e, pause_ms).await })
}

/// Остаток после частичной продажи кнопкой (manual_sell::remainder, решение 235). Принимает: ядро, сделку.
/// Возвращает: продолжение. Ошибок нет.
#[inline(never)]
pub fn button_remainder(core: &Core, deal: Deal) -> Job {
    let c = core.clone();
    Box::pin(async move { super::manual_sell::remainder(&c, deal).await })
}

/// Газ BNB продажи кнопкой не помещается сверх резерва — алерт (manual_sell::gas, решение 235). Принимает: ядро,
/// сделку, ошибку. Возвращает: продолжение. Ошибок нет.
#[inline(never)]
pub fn button_gas(core: &Core, deal: Deal, e: &OpError) -> Job {
    let (c, e) = (core.clone(), e.clone());
    Box::pin(async move { super::manual_sell::gas(&c, deal, &e).await })
}
