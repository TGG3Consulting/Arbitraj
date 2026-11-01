//! Файл: core/commands.rs
//! Что делает: команды человека ядру — из интерфейса и кнопок Telegram (ManualCommand, core/types/control.rs):
//! «Продать сейчас», «Беру на себя», «Вернуть боту»; для остановленной сделки — «Продать сейчас» и «Продано вручную»
//! (manual_halted.rs, решение 100). Кнопка Telegram принимается только из настроенного
//! чата notify.telegram_chat (решение 37). Каждая команда — в журнал с источником. Плюс команды экрана
//! «Управление»: старт, стоп, пауза системы, снятие паузы площадки и карантина монеты.
//! Кто вызывает: api (POST /api/manual/command, экран «Управление») и notify (кнопки Telegram) — через методы Core.
//! От чего зависит: manual.rs, risk.rs, settings, deps.rs.
//! Настройки: notify.telegram_chat.

use serde_json::json;

use crate::core::types::{Asset, CommandSource, ManualAction, ManualCommand, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::risk::{self, Mode};

impl Core {
    /// Выполняет команду человека. Возвращает: Ok(текст) — выполнено; Err(причина) — отклонено.
    /// Обе ветки пишутся в журнал с источником команды.
    pub async fn command(&self, cmd: ManualCommand) -> Result<String, String> {
        let res = self.run_command(&cmd).await;
        let (level, text) = match &res {
            Ok(t) => (
                Level::Info,
                format!("Команда {:?} ({:?}): {t}", cmd.action, cmd.source),
            ),
            Err(e) => (
                Level::Warn,
                format!("Команда {:?} ({:?}) отклонена: {e}", cmd.action, cmd.source),
            ),
        };
        self.inner
            .deps
            .log(level, "core.command", None, text, Some(json!(cmd)))
            .await;
        res
    }

    /// Проверка источника и выполнение.
    async fn run_command(&self, cmd: &ManualCommand) -> Result<String, String> {
        if let CommandSource::Telegram { chat } = &cmd.source {
            let ours = self
                .inner
                .deps
                .settings
                .text("notify.telegram_chat", At::default())
                .map_err(|e| e.message)?;
            // Настройка может быть @username чата — тогда совпадение уже проверил notify (press::chat_matches),
            // у ядра есть только числовой id.
            if chat != &ours && !ours.starts_with('@') {
                return Err(format!("чат {chat} — не настроенный чат уведомлений"));
            }
        }
        // Команда по уже закрытому алерту (или вернувшимся монетам) — спокойно игнорируется (Issue #10).
        let gone = match &cmd.action {
            ManualAction::SellNow { alert_id } | ManualAction::TakeOver { alert_id } => {
                (!lk(&self.inner.alerts).contains_key(alert_id))
                    .then(|| format!("алерт {alert_id}"))
            }
            ManualAction::ReturnToBot { holding_id } => (!lk(&self.inner.holdings)
                .contains_key(holding_id))
            .then(|| format!("запись {holding_id}")),
            // Шаг сделки проверяют сами команды (manual_halted.rs) — отказ с причиной.
            ManualAction::SellHalted { .. } | ManualAction::SoldManually { .. } => None,
        };
        if let Some(what) = gone {
            return Ok(format!(
                "{what} уже закрыт(а) или не существует — команда проигнорирована"
            ));
        }
        let who = match (&cmd.source, &cmd.by) {
            (CommandSource::Ui, _) => "интерфейс".to_string(),
            (CommandSource::Telegram { chat }, Some(by)) => format!("Telegram {chat}, {by}"),
            (CommandSource::Telegram { chat }, None) => format!("Telegram {chat}"),
        };
        match &cmd.action {
            ManualAction::SellNow { alert_id } => super::manual::sell_now(self, alert_id).await,
            ManualAction::TakeOver { alert_id } => {
                super::manual::take_over(self, alert_id, &who).await
            }
            ManualAction::ReturnToBot { holding_id } => {
                super::manual_return::return_to_bot(self, holding_id).await
            }
            ManualAction::SellHalted { deal_id } => {
                super::manual_halted::sell_halted(self, deal_id).await
            }
            ManualAction::SoldManually { deal_id, usdt } => {
                super::manual_halted::sold_manually(self, deal_id, usdt, &who).await
            }
        }
    }

    /// Экран «Управление»: режим системы (Running — «Старт», Stopped — «Стоп», Paused — «Пауза»).
    /// Возвращает: текст для журнала. Ошибок нет.
    pub async fn set_mode(&self, mode: Mode) -> String {
        let t = risk::set_mode(self, mode, Some("команда из интерфейса".into()));
        self.inner
            .deps
            .log(Level::Info, "core.control", None, &t, None)
            .await;
        let code = if mode == Mode::Running {
            "system.started"
        } else {
            "system.stopped"
        };
        self.inner.deps.notify(code, None, &t).await;
        t
    }

    /// Снять паузу площадки (после ошибок подряд или правила шлюза).
    pub async fn resume_venue(&self, venue: &VenueId) {
        risk::resume_venue(self, venue);
        self.inner
            .deps
            .log(
                Level::Info,
                "core.control",
                Some(&venue.0),
                "Пауза площадки снята",
                None,
            )
            .await;
    }

    /// Снять карантин монеты (ручная проверка после «депозит не зачислен» или медленного перевода).
    pub async fn release_quarantine(&self, asset: &Asset) {
        risk::quarantine(self, asset, None);
        let t = format!("{}: карантин снят вручную", asset.ticker);
        self.inner
            .deps
            .log(Level::Info, "core.control", None, &t, None)
            .await;
        self.inner
            .deps
            .notify("route.quarantine_out", None, &t)
            .await;
    }

    /// Состояние экрана «Управление».
    pub fn control(&self) -> risk::Risk {
        risk::snapshot(self)
    }
}

impl Core {
    /// Канал команд для кнопок Telegram (core/types/control.rs, CommandTx): каждая команда выполняется
    /// по очереди через Core::command. Размер очереди — --live-buffer. Ошибок нет.
    pub fn command_channel(&self) -> crate::core::types::CommandTx {
        let (tx, mut rx) = tokio::sync::mpsc::channel(self.inner.deps.buffer);
        let core = self.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                let _ = core.command(cmd).await;
            }
        });
        tx
    }
}
