//! Файл: core/drawdown_cfg.rs
//! Что делает: решение 414 — пределы просадки направления из настроек: risk.drawdown_pct (X % от пика) и
//! risk.drawdown_max_usd (Y USDT). Порядок чтения — как у лимита пары решения 258 (core/day_pair.rs max; решение 242):
//! значение пары «площадка × тип направления» → значение площадки (биржа пары; у «DEX ↔ DEX» — DEX реестра) → вся
//! система. Значений по умолчанию нет.
//! Замена старого механизма (п. 9 постановки t199): направление, у которого задана ХОТЯ БЫ ОДНА из двух настроек
//! (на любом уровне), считается по просадке от пика, а старый лимит risk.max_daily_loss_usd для него не действует.
//! Обе не заданы ни на одном уровне — направление работает по-старому (risk.max_daily_loss_usd, решение 258): так
//! выкладка не меняет поведение, пока Тигран не задал новые пределы; не задано ничего — торговля направлением не
//! включается (risk_ready.rs), как раньше.
//! Кто вызывает: drawdown_limit.rs, drawdown_snap.rs, risk_ready.rs, daily_venue_limit.rs, daily_limit.rs, day_roll.rs,
//! risk.rs (snapshot).
//! От чего зависит: day_pair.rs (venue_or_dex), settings (Store::decimal_dir), drawdown.rs (Limit).
//! Настройки: risk.drawdown_pct, risk.drawdown_max_usd.

use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::settings::venue_dir::{VenueDir, parse_id};

use super::drawdown::Limit;
use super::engine::Core;

/// Предел просадки в процентах от пика.
pub const PCT: &str = "risk.drawdown_pct";
/// Предел просадки в USDT.
pub const USD: &str = "risk.drawdown_max_usd";
/// Общий префикс (наблюдатель изменений настроек, drawdown_snap.rs).
pub const PREFIX: &str = "risk.drawdown_";

/// Что делает: значение настройки у пары: пара → площадка → вся система. Принимает: ядро, id пары, имя.
/// Возвращает: None — не задана нигде (или не число). Ошибок нет.
fn read(core: &Core, p: &str, name: &str) -> Option<Decimal> {
    let v = super::day_pair::venue_or_dex(core, p);
    let vd = parse_id(p).map(|(venue, kind)| VenueDir {
        venue: venue.unwrap_or(""),
        kind,
    });
    let at = At {
        venue: v.as_deref(),
        coin: None,
    };
    core.inner.deps.settings.decimal_dir(name, vd, at).ok()
}

/// Что делает: пределы просадки пары. Принимает: ядро, id пары. Возвращает: None — обе настройки не заданы (пара — по
/// старому лимиту дня). Ошибок нет.
pub fn limit(core: &Core, p: &str) -> Option<Limit> {
    let pct = read(core, p, PCT);
    let usd = read(core, p, USD);
    if pct.is_none() && usd.is_none() {
        return None;
    }
    Some(Limit { pct, usd })
}

/// Направление считается по просадке от пика (задана хотя бы одна из двух настроек). Ошибок нет.
pub fn active(core: &Core, p: &str) -> bool {
    limit(core, p).is_some()
}
