//! Файл: core/low_usdt.rs
//! Что делает: алерт «мало USDT» (решение 54): свободных USDT на площадке ≤ alert.min_usdt этой площадки →
//! событие risk.low_usdt в Telegram один раз на пересечение порога; повтор — только после подъёма выше
//! порога и нового падения. Состояние «ниже порога» по площадке хранится в учёте ядра (core_state,
//! вид «low_usdt») — после перезапуска сохраняется, повторного алерта нет. Не задано — алерта нет,
//! в журнал одна запись «не задано» на площадку (до следующего перезапуска).
//! Имя не из группы risk.*: решение 15 не включает торговлю без всех risk.*, а алерт не должен её держать.
//! Автоперегон USDT (usdt_return.rs, решение 34) этим не меняется.
//! Кто вызывает: core/reconcile.rs (каждая сверка балансов), core/engine.rs (load).
//! От чего зависит: engine.rs, persist.rs, settings.
//! Настройки: alert.min_usdt (по площадке).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::scope::Scope;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Вид записи в учёте ядра.
const KIND: &str = "low_usdt";

/// Восстанавливает состояние «ниже порога» по площадкам при запуске. Ошибка базы — в журнал.
pub async fn load(core: &Core) {
    match super::persist::load::<(VenueId, bool)>(&core.inner.deps.db, KIND).await {
        Ok(list) => lk(&core.inner.usdt_below).extend(list),
        Err(e) => {
            let m = format!("Состояние алерта «мало USDT» не загружено: {e}");
            core.inner
                .deps
                .log(Level::Error, "core.low_usdt", None, m, None)
                .await;
        }
    }
}

/// Проверка одной площадки. Принимает: ядро, площадку, свободно USDT, тикер котируемой монеты.
/// Ошибок нет: всё — в журнал и Telegram.
pub async fn check(core: &Core, venue: &VenueId, free: Decimal, quote: &str) {
    let deps = &core.inner.deps;
    let min = match deps
        .settings
        .decimal_in("alert.min_usdt", &Scope::Venue(venue.0.clone()))
    {
        Ok(m) => m,
        Err(e) => {
            if lk(&core.inner.usdt_alert_unset).insert(venue.clone()) {
                deps.log(
                    Level::Warn,
                    "core.low_usdt",
                    Some(&venue.0),
                    format!("Алерт «мало USDT» не работает: {}", e.message),
                    None,
                )
                .await;
            }
            return;
        }
    };
    lk(&core.inner.usdt_alert_unset).remove(venue);
    let below = free <= min;
    let was = lk(&core.inner.usdt_below).insert(venue.clone(), below);
    if was == Some(below) {
        return;
    }
    if let Err(e) = super::persist::save(&deps.db, KIND, &venue.0, &(venue.clone(), below)).await {
        deps.log(
            Level::Error,
            "core.low_usdt",
            Some(&venue.0),
            format!("Состояние алерта не сохранено: {e}"),
            None,
        )
        .await;
    }
    if below {
        let t = format!(
            "{}: свободно {free} {quote} — не больше порога алерта {min}",
            venue.0
        );
        deps.log(Level::Warn, "core.low_usdt", Some(&venue.0), &t, None)
            .await;
        deps.notify("risk.low_usdt", Some(&venue.0), &t).await;
    } else if was.is_some() {
        let t = format!(
            "{}: свободно {free} {quote} — снова выше порога алерта {min}",
            venue.0
        );
        deps.log(Level::Info, "core.low_usdt", Some(&venue.0), t, None)
            .await;
    }
}
