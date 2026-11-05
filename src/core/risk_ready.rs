//! Файл: core/risk_ready.rs
//! Что делает: заданы ли настройки, без которых торговля по направлению не включается (вынесено из risk.rs):
//! все обязательные money.* и risk.* (решение 15; необязательные — например risk.max_daily_loss_total_usd — не
//! требуются), risk.max_errors_in_row у биржи продажи (решение 118, Я-10), risk.max_daily_loss_usd у каждой площадки
//! учёта дня сделки (решение 173: значение площадки или «вся система»).
//! Кто вызывает: risk.rs (can_open).
//! От чего зависит: settings (catalog, store), daily_venue.rs (площадки сделки).
//! Настройки: все обязательные money.* и risk.*, risk.max_errors_in_row, risk.max_daily_loss_usd.

use crate::core::types::Direction;
use crate::settings::catalog;

use super::engine::Core;

/// Что делает: проверка настроек направления. Принимает: ядро, направление.
/// Возвращает: Ok или Err — «торговля не включается: не задана настройка …».
pub(super) fn settings_ready(core: &Core, d: &Direction) -> Result<(), String> {
    let s = &core.inner.deps.settings;
    for def in catalog::all()
        .filter(|d| !d.optional && (d.name.starts_with("money.") || d.name.starts_with("risk.")))
    {
        if s.resolve(def.name, Some(&d.buy_on.0), None).is_none() {
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
    // Решение 173: у каждой площадки учёта сделки — свой лимит дня (значение площадки или «вся система»).
    for v in super::daily_venue::of_direction(core, d) {
        if s.resolve("risk.max_daily_loss_usd", Some(&v), None)
            .is_none()
        {
            return Err(format!(
                "торговля не включается: не задана настройка risk.max_daily_loss_usd [venue:{v}]"
            ));
        }
    }
    Ok(())
}
