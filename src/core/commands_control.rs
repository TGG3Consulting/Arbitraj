//! Файл: core/commands_control.rs
//! Что делает: команды плашки режима (решение 157) через общий путь команд ядра (Core::command): «Старт» / «Пауза» /
//! «Стоп», «Снять паузу» площадки, «Снять карантин» монеты, «Сбросить лимит дня» — те же методы ядра, что вызывает
//! интерфейс (api/core_api.rs: set_mode, resume_venue, release_quarantine, reset_daily_limit), второй логики нет.
//! Кто вызывает: core/commands.rs (run_command) — для команд из Telegram (меню /menu и короткие /start /stop …).
//! От чего зависит: commands.rs, daily_limit.rs, engine.rs.
//! Настройки: нет.

use crate::core::types::{ManualAction, VenueId};

use super::engine::Core;
use super::risk::Mode;

/// Выполняет команду плашки режима. Принимает: ядро, действие, кто (commands::who).
/// Возвращает: Ok(текст итога) или Err — действие не из плашки режима (сюда не должно приходить).
pub async fn run(core: &Core, action: &ManualAction, who: &str) -> Result<String, String> {
    match action {
        ManualAction::SetMode { mode } => {
            let t = core.set_mode_by(*mode, &format!("команда: {who}")).await;
            Ok(format!("режим {}: {t}", label(*mode)))
        }
        ManualAction::ResumeVenue { venue } => {
            let paused = core
                .control()
                .venue_paused
                .contains_key(&VenueId(venue.clone()));
            if !paused {
                return Ok(format!(
                    "площадка {venue} не на паузе — команда проигнорирована"
                ));
            }
            core.resume_venue(&VenueId(venue.clone())).await;
            Ok(format!("пауза площадки {venue} снята"))
        }
        ManualAction::ReleaseQuarantine { id } => {
            core.release_quarantine(id).await;
            Ok(format!("карантин {id} снят"))
        }
        ManualAction::ResetDailyLimit => Ok(core.reset_daily_limit(who).await),
        other => Err(format!("{other:?} — не команда плашки режима")),
    }
}

/// Название режима — как на плашке (ui/src/lib/coreLabels.js, MODE). Принимает: режим.
pub fn label(m: Mode) -> &'static str {
    match m {
        Mode::Stopped => "Стоп",
        Mode::Running => "В работе",
        Mode::Paused => "Пауза",
    }
}
