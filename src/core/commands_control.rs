//! Файл: core/commands_control.rs
//! Что делает: команды плашки режима (решение 157) через общий путь команд ядра (Core::command): «Старт» / «Пауза» /
//! «Стоп», «Снять паузу» площадки и кошелька DEX (решение 230), «Снять карантин» монеты, «Сбросить лимит дня» —
//! общий и по площадке (решение 173) — те же методы ядра, что вызывает интерфейс (api/core_api.rs: set_mode,
//! resume_venue, resume_wallet, release_quarantine, reset_daily_limit, reset_venue_daily_limit), второй логики нет.
//! Кто вызывает: core/commands.rs (run_command) — для команд из Telegram (меню /menu и короткие /start /stop …).
//! От чего зависит: commands.rs, daily_limit.rs, daily_venue_limit.rs, engine.rs, wallet_pause.rs.
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
            let r = core.control();
            // Решение 173: у DEX может стоять пауза только циклов DEX ↔ DEX (cycle_paused).
            let paused = r.venue_paused.contains_key(&VenueId(venue.clone()))
                || r.cycle_paused.contains_key(venue);
            if !paused {
                return Ok(format!(
                    "площадка {venue} не на паузе — команда проигнорирована"
                ));
            }
            core.resume_venue(&VenueId(venue.clone())).await;
            Ok(format!("пауза площадки {venue} снята"))
        }
        // Решение 230: пауза кошелька DEX — Core::resume_wallet (журнал core.control с именем кошелька, как у кнопки
        // плашки); кто снял — в журнале core.command (как у площадки).
        ManualAction::ResumeWallet { wallet } => {
            let Some(p) = core.control().wallet_paused.get(wallet).cloned() else {
                return Ok(format!(
                    "кошелёк {wallet} не на паузе — команда проигнорирована"
                ));
            };
            if !core.resume_wallet(wallet).await {
                return Ok(format!(
                    "{} уже не на паузе — команда проигнорирована",
                    p.who()
                ));
            }
            Ok(format!("пауза кошелька «{}» снята", p.name))
        }
        ManualAction::ReleaseQuarantine { id } => {
            core.release_quarantine(id).await;
            Ok(format!("карантин {id} снят"))
        }
        ManualAction::ResetDailyLimit => Ok(core.reset_daily_limit(who).await),
        ManualAction::ResetVenueLimit { venue } => {
            Ok(core.reset_venue_daily_limit(venue, who).await)
        }
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
