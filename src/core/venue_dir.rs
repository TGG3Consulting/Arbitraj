//! Файл: core/venue_dir.rs
//! Что делает: решение 242 — у какой площадки и какого типа направления берётся значение настроек денег и прибыли
//! сделки (правило стороны) и чтение порогов прибыли с этой областью. Одна функция для всех мест чтения
//! money.min_profit_usd, money.min_profit_pct, money.max_per_trade_usd, profit.price_move_k, profit.closure_share.
//! Правило стороны (Тигран 09.11, решение 242): типы направлений описаны у биржи («у BingX 3 направления: CEX-CEX,
//! CEX-DEX, DEX-CEX»), у DEX — «DEX ↔ DEX»:
//! - биржа → DEX (покупка на бирже, продажа на DEX) — значение биржи, тип «биржа → DEX»;
//! - DEX → биржа (покупка на DEX, продажа на бирже) — значение БИРЖИ (площадка продажи), тип «DEX → биржа»;
//! - биржа → биржа — значение биржи ПОКУПКИ (buy_on, инициатор), тип «биржа → биржа»;
//! - DEX ↔ DEX (цикл: покупка и продажа на одной DEX) — тип «DEX ↔ DEX»: один отдельный набор на все сделки DEX ↔ DEX
//!   (Тигран 09.11: не по DEX и не по кошельку; площадка в область не входит — settings/venue_dir.rs id).
//!
//! Вид площадки неизвестен (нет в шлюзе) — области нет: действует прежняя цепочка.
//! Пустая область — чтение ровно прежнее (settings/venue_dir.rs: decimal_dir → decimal).
//! Кто вызывает: core/profit.rs, card_max.rs, uncapped.rs, precheck_dex_sell.rs, prices.rs, limits_parts.rs,
//! cycle_plan.rs, dirctx.rs, reserve.rs, risk_ready.rs, uncapped_recompute_ctx.rs.
//! От чего зависит: engine.rs (виды площадок), gateway/contract.rs (VenueKind), settings/venue_dir.rs.
//! Настройки: money.min_profit_usd, money.min_profit_pct (thresholds).

use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::settings::store::Store;
pub use crate::settings::venue_dir::{DirKind, VenueDir};

use super::engine::{Core, rd};

/// Что делает: правило стороны (см. шапку). Принимает: направление, вид площадки покупки и продажи (None — неизвестен).
/// Возвращает: площадку и тип направления или None — вид неизвестен (действует прежняя цепочка). Ошибок нет.
pub fn rule(
    d: &Direction,
    buy: Option<VenueKind>,
    sell: Option<VenueKind>,
) -> Option<VenueDir<'_>> {
    use VenueKind::{Cex, Dex};
    let (venue, kind) = match (buy?, sell?) {
        (Cex, Dex) => (&d.buy_on, DirKind::CexToDex),
        (Dex, Cex) => (&d.sell_on, DirKind::DexToCex),
        (Cex, Cex) => (&d.buy_on, DirKind::CexCex),
        (Dex, Dex) => (&d.buy_on, DirKind::DexDex),
    };
    Some(VenueDir {
        venue: &venue.0,
        kind,
    })
}

/// Что делает: площадка и тип направления сделки по видам площадок шлюза (rule). Принимает: ядро, направление.
/// Возвращает: пару или None — вид площадки неизвестен. Ошибок нет.
pub fn of<'a>(core: &Core, d: &'a Direction) -> Option<VenueDir<'a>> {
    let kinds = rd(&core.inner.kinds);
    let kind =
        |v: &crate::core::types::VenueId| kinds.iter().find(|(id, _)| id == v).map(|(_, k)| *k);
    rule(d, kind(&d.buy_on), kind(&d.sell_on))
}

/// Что делает: пороги прибыли сделки money.min_profit_usd и money.min_profit_pct — значение пары «площадка + тип
/// направления», иначе вся система (как раньше). Принимает: хранилище (живое или копия момента входа), пару.
/// Возвращает: (порог USDT, порог %). Ошибки: текст — не задан (сначала порог USDT) или неверный вид.
pub fn thresholds(s: &Store, vd: Option<VenueDir>) -> Result<(Decimal, Decimal), String> {
    let g = |name: &str| {
        s.decimal_dir(name, vd, At::default())
            .map_err(|e| e.message)
    };
    Ok((g("money.min_profit_usd")?, g("money.min_profit_pct")?))
}
