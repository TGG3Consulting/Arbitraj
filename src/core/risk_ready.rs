//! Файл: core/risk_ready.rs
//! Что делает: заданы ли настройки, без которых торговля по направлению не включается (вынесено из risk.rs):
//! все обязательные money.* и risk.* (решение 15; необязательные — например risk.max_daily_loss_total_usd — не
//! требуются), risk.max_errors_in_row у биржи продажи (решение 118, Я-10), risk.max_daily_loss_usd у пары сделки
//! «площадка × тип направления» (решение 258: значение пары → площадки → вся система, day_pair.rs — тот же порядок, что
//! у проверки лимита).
//! Кто вызывает: risk.rs (can_open).
//! От чего зависит: settings (catalog, store), day_pair.rs (пара сделки и её лимит).
//! Решение 242: money.* с областью «площадка + тип направления» — заданы и значением пары (core/venue_dir.rs).
//! Настройки: все обязательные money.* и risk.*, risk.max_errors_in_row, risk.max_daily_loss_usd.

use crate::core::types::Direction;
use crate::settings::catalog;
use crate::settings::schema::ScopeKind;

use super::engine::Core;

/// Что делает: проверка настроек направления. Принимает: ядро, направление.
/// Возвращает: Ok или Err — «торговля не включается: не задана настройка …».
pub(super) fn settings_ready(core: &Core, d: &Direction) -> Result<(), String> {
    let s = &core.inner.deps.settings;
    // Решение 242: у настроек с областью «площадка + тип направления» значение пары тоже считается заданным (как его
    // читает расчёт — venue_dir.rs); не задано для пары — как раньше.
    let vd = super::venue_dir::of(core, d);
    // Решение 258: risk.max_daily_loss_usd — ниже, у пары сделки (площадка второго шага — биржа пары, а не площадка
    // покупки, как у остальных).
    for def in catalog::all().filter(|d| {
        !d.optional
            && d.name != super::day_pair::SETTING
            && (d.name.starts_with("money.") || d.name.starts_with("risk."))
    }) {
        let pair = vd.filter(|_| def.scopes.contains(&ScopeKind::VenueDir));
        if s.resolve_dir(def.name, pair, Some(&d.buy_on.0), None)
            .is_none()
        {
            return Err(format!(
                "торговля не включается: не задана настройка {}",
                def.name
            ));
        }
    }
    // Решение 118 (Я-10): у биржи продажи — своя пауза после ошибок подряд; без неё B на сбое не встанет на паузу.
    if s.resolve("risk.max_errors_in_row", Some(&d.sell_on.0), None)
        .is_none()
    {
        return Err(format!(
            "торговля не включается: не задана настройка risk.max_errors_in_row [venue:{}]",
            d.sell_on.0
        ));
    }
    // Решение 258: у пары сделки — свой лимит дня (значение пары, площадки или «вся система»).
    let pair = super::day_pair::of_direction(core, d);
    if !super::day_pair::is_set(core, &pair) {
        return Err(format!(
            "торговля не включается: не задана настройка {} [venue_dir:{pair}] — ни у направления {}, ни у площадки, ни у всей системы",
            super::day_pair::SETTING,
            super::day_pair::label(&pair)
        ));
    }
    Ok(())
}
