//! Файл: core/signal_ctx.rs
//! Что делает: обстоятельства пересчёта направления для сигнала (решения 317, 318): какая сторона направления — DEX
//! (по виду площадки из реестра шлюза, VenueKind — без знания конкретных площадок) и что запустило пересчёт:
//! событие пула DEX (площадка DEX направления прислала тик — у DEX тик бывает только от MarketEvent::Pool,
//! core/tick.rs Touched::Pool), таймер дозревания (решение 318) или тик биржи.
//! Решение 317 (доработка): сколько мс стоят уровни стакана биржи направления на объёме сравнения — из отметок
//! изменения содержимого стакана в состоянии рынка ядра (market_read.rs book_still_since, book_still.rs).
//! Кто вызывает: core/evaluate.rs на каждое направление пересчёта.
//! От чего зависит: core/engine.rs (Inner.kinds, Inner.market), core/venue_ticker.rs, core/signal_fast.rs (Ctx,
//! Trigger), gateway/contract (VenueKind).
//! Настройки: нет.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Direction, Side, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, rd};
use super::signal_fast::{Ctx, Trigger};

/// Что делает: обстоятельства пересчёта направления.
/// Принимает: ядро, направление, площадки, приславшие тики к этому пересчёту, пришёл ли пересчёт по таймеру
/// (решение 318). Возвращает: Ctx. Ошибок нет.
///
/// Порядок пометки: событие пула DEX направления → «событие пула» (у биржи ↔ DEX — даже если вместе с ним сработал
/// таймер: тик пула настоящий); иначе таймер → «таймер»; иначе «тик биржи». Тик биржи, слитый с таймером в один пересчёт
/// (решение 109), помечается «таймер» — на правило 317 это не влияет (оба — обычное удержание).
pub fn of(core: &Core, dir: &Direction, venues: &HashSet<VenueId>, timer: bool) -> Ctx {
    let (a_dex, b_dex) = {
        let kinds = rd(&core.inner.kinds);
        let dex = |v: &VenueId| {
            kinds
                .iter()
                .any(|(k, kind)| k == v && *kind == VenueKind::Dex)
        };
        (dex(&dir.buy_on), dex(&dir.sell_on))
    };
    let dex_side = match (a_dex, b_dex) {
        (true, false) => Some(Side::Buy),
        (false, true) => Some(Side::Sell),
        _ => None,
    };
    let ticked =
        (a_dex && venues.contains(&dir.buy_on)) || (b_dex && venues.contains(&dir.sell_on));
    // Биржа ↔ DEX: таймер ставит в пересчёт площадку биржи (signal_ripen.rs), поэтому площадка DEX в тиках — только
    // от события пула. DEX ↔ DEX: таймер ставит площадку DEX — при таймере пометка «таймер» (правило 317 тут не
    // действует, это только подпись журнала).
    let pool = ticked && (dex_side.is_some() || !timer);
    let trigger = if pool {
        Trigger::Pool
    } else if timer {
        Trigger::Timer
    } else {
        Trigger::Book
    };
    Ctx {
        dex_side,
        trigger,
        ..Ctx::default()
    }
}

/// Что делает: сколько мс не менялись уровни стакана биржи направления биржа ↔ DEX, которыми котировка набирает объём.
/// Принимает: ядро, направление, обстоятельства (сторона DEX), объём сравнения (Prices.qty), время пересчёта.
/// Возвращает: мс; None — направление не биржа ↔ DEX или стакана биржи нет в состоянии рынка. Ошибок нет.
pub fn cex_still_ms(
    core: &Core,
    dir: &Direction,
    ctx: &Ctx,
    qty: Decimal,
    now: DateTime<Utc>,
) -> Option<u64> {
    // Сторона DEX — покупка: на бирже продаём (bids); сторона DEX — продажа: на бирже покупаем (asks).
    let (venue, side) = match ctx.dex_side? {
        Side::Buy => (&dir.sell_on, Side::Sell),
        Side::Sell => (&dir.buy_on, Side::Buy),
    };
    let ticker = super::venue_ticker::ticker_on(core, &dir.asset, venue);
    let since = rd(&core.inner.market).book_still_since(venue, &ticker, side, qty)?;
    Some((now - since).num_milliseconds().max(0) as u64)
}
