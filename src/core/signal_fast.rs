//! Файл: core/signal_fast.rs
//! Что делает: решение 317 — сигнал от события пула без удержания. Помнит по направлению биржа ↔ DEX цену стороны
//! биржи (средняя цена котировки, по которой считается сделка) и с какого пересчёта она не менялась, и последнюю цену
//! стороны DEX. Правило: пересчёт запущен событием пула DEX (MarketEvent::Pool → Trigger::Pool), цена стороны DEX
//! изменилась с прошлого пересчёта, а цена стороны биржи не менялась не меньше signal.min_hold_ms — защита от
//! мигающего стакана (призрачного уровня) уже выполнена временем, кандидат выдаётся с первого обновления.
//! Биржа ↔ биржа и DEX ↔ DEX — правило не действует (dex_side = None).
//!
//! Кто вызывает: core/signal.rs (Signals::update_with), core/evaluate.rs (Ctx — что запустило пересчёт).
//! От чего зависит: core/types (Side), chrono, rust_decimal.
//! Настройки: signal.dex_event_skip_hold (читает signal.rs Params; не задана — правило выключено).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

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

/// Обстоятельства пересчёта направления, известные ядру (evaluate.rs).
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx {
    /// Сторона направления на DEX, если ровно одна сторона — DEX (биржа ↔ DEX); None — биржа ↔ биржа, DEX ↔ DEX.
    pub dex_side: Option<Side>,
    /// Что запустило пересчёт.
    pub trigger: Trigger,
}

/// Память сторон направления биржа ↔ DEX.
#[derive(Debug, Clone, Default)]
pub struct Sides {
    /// Цена стороны биржи и время пересчёта, на котором она впервые стала такой.
    cex: Option<(Decimal, DateTime<Utc>)>,
    /// Цена стороны DEX на прошлом пересчёте.
    dex: Option<Decimal>,
}

/// Итог наблюдения сторон на пересчёте.
#[derive(Debug, Clone, Copy)]
pub struct Seen {
    /// Цена стороны DEX отличается от прошлого пересчёта (прошлого нет — false).
    pub dex_changed: bool,
    /// Сколько мс цена стороны биржи не менялась (от первого пересчёта с этой ценой до сейчас).
    pub cex_still_ms: u64,
}

impl Sides {
    /// Что делает: запоминает цены сторон пересчёта и сравнивает с прошлым.
    /// Принимает: обстоятельства пересчёта, среднюю цену покупки на A и продажи на B, время пересчёта.
    /// Возвращает: Seen; None — направление не биржа ↔ DEX (память не ведётся). Ошибок нет.
    pub fn observe(
        &mut self,
        ctx: &Ctx,
        buy: Decimal,
        sell: Decimal,
        now: DateTime<Utc>,
    ) -> Option<Seen> {
        let side = ctx.dex_side?;
        let (dex, cex) = if side == Side::Buy {
            (buy, sell)
        } else {
            (sell, buy)
        };
        // Цена биржи та же — отметка остаётся с первого пересчёта, где она была такой; иначе — с этого пересчёта.
        let since = match self.cex {
            Some((p, s)) if p == cex => s,
            _ => now,
        };
        self.cex = Some((cex, since));
        let dex_changed = self.dex.is_some_and(|p| p != dex);
        self.dex = Some(dex);
        Some(Seen {
            dex_changed,
            cex_still_ms: (now - since).num_milliseconds().max(0) as u64,
        })
    }
}

/// Что делает: правило решения 317 — пропустить удержание.
/// Принимает: включено ли (signal.dex_event_skip_hold), signal.min_hold_ms, обстоятельства, наблюдение сторон.
/// Возвращает: true — кандидат с первого обновления: пересчёт от события пула, цена DEX изменилась, цена биржи
/// не менялась не меньше hold_ms. Ошибок нет.
pub fn skip_hold(enabled: bool, hold_ms: u64, ctx: &Ctx, seen: Option<&Seen>) -> bool {
    enabled
        && ctx.trigger == Trigger::Pool
        && seen.is_some_and(|s| s.dex_changed && s.cex_still_ms >= hold_ms)
}
