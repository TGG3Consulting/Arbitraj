//! Файл: core/signal_fast.rs
//! Что делает: решение 317 (с доработкой 16.11) — сигнал от события пула без удержания. Правило: направление биржа ↔
//! DEX, пересчёт запущен событием пула по площадке DEX направления (Trigger::Pool — факт цепи: сторона DEX изменилась),
//! а уровни стакана биржи, которыми котировка набирает объём сравнения, не менялись не меньше signal.min_hold_ms
//! (Ctx.cex_still_ms — из отметок изменения содержимого стакана в состоянии рынка ядра, core/book_still.rs, а не из
//! памяти прошлых пересчётов) — защита от мигающего стакана (призрачного уровня) уже выполнена временем, кандидат
//! выдаётся с первого обновления. Работает на первом же событии пула: после запуска, после «нет цены», при смене
//! объёма сравнения. Биржа ↔ биржа и DEX ↔ DEX — правило не действует (dex_side = None).
//!
//! Кто вызывает: core/signal.rs (Signals::update_with), core/evaluate.rs и core/signal_ctx.rs (Ctx).
//! От чего зависит: core/types (Side), core/tick_plan.rs (PoolMark — замер), chrono.
//! Настройки: signal.dex_event_skip_hold (читает signal.rs Params; не задана — правило выключено).

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::tick_plan::PoolMark;
use crate::core::types::Side;

/// Что запустило пересчёт направления (журнал core.signal_hold и «Живой экран»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub enum Trigger {
    /// Тик биржи: стакан по WebSocket или ответ REST-стакана (feed_rest_book.rs).
    #[default]
    Book,
    /// Событие пула DEX (MarketEvent::Pool) по площадке DEX этого направления.
    Pool,
    /// Таймер дозревания (решение 318, signal_ripen.rs): новых тиков не было.
    Timer,
}

/// Обстоятельства пересчёта направления, известные ядру (evaluate.rs, signal_ctx.rs).
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx {
    /// Сторона направления на DEX, если ровно одна сторона — DEX (биржа ↔ DEX); None — биржа ↔ биржа, DEX ↔ DEX.
    pub dex_side: Option<Side>,
    /// Что запустило пересчёт.
    pub trigger: Trigger,
    /// Сколько мс не менялись уровни стакана биржи на объёме сравнения (только биржа ↔ DEX и есть цена; None — нет
    /// стакана или направление не биржа ↔ DEX).
    pub cex_still_ms: Option<u64>,
    /// Замер (запись core.signal_hold): первое событие пула пересчёта — когда пришло в поток тика (tick.rs) и номер
    /// блока; только при пересчёте от события пула по DEX направления (Trigger::Pool).
    pub pool: Option<PoolMark>,
    /// Замер: когда начался пересчёт монеты (tick_sched.rs).
    pub started: Option<DateTime<Utc>>,
}

/// Что делает: правило решения 317 — пропустить удержание.
/// Принимает: включено ли (signal.dex_event_skip_hold), signal.min_hold_ms, обстоятельства пересчёта.
/// Возвращает: true — кандидат с первого обновления: биржа ↔ DEX, пересчёт от события пула по площадке DEX, уровни
/// стакана биржи на объёме сравнения стоят не меньше hold_ms. Ошибок нет.
pub fn skip_hold(enabled: bool, hold_ms: u64, ctx: &Ctx) -> bool {
    enabled
        && ctx.dex_side.is_some()
        && ctx.trigger == Trigger::Pool
        && ctx.cex_still_ms.is_some_and(|ms| ms >= hold_ms)
}
