//! Файл: core/commands.rs
//! Что делает: команды человека ядру — из интерфейса и кнопок Telegram (ManualCommand, core/types/control.rs):
//! «Продать сейчас», «Беру на себя», «Вернуть боту»; для остановленной сделки — «Продать сейчас» и «Продано вручную»
//! (manual_halted.rs, решение 100); «Продать и перегнать» — у алерта и остановленной сделки (решение 101); «Перегнать» и
//! «Перегнано вручную» у записи «USDT не вернулись» (manual_usdt.rs, решение 163). Кнопка Telegram принимается только из чатов ролей
//! (решения 37, 176). Каждая команда — в журнал с источником. Плюс команды экрана
//! «Управление»: старт, стоп, пауза системы, снятие паузы площадки и карантина монеты (решение 157: те же команды
//! из Telegram — ManualAction::SetMode и др., commands_control.rs). Итог каждой команды — событие CoreEvent::Command.
//! Решение 177: команда выполняется с признаком «ручная» (risk_count.rs) — не считается ошибкой площадки подряд.
//! Кто вызывает: api (POST /api/manual/command, экран «Управление») и notify (кнопки Telegram) — через методы Core.
//! От чего зависит: manual.rs, commands_control.rs, risk.rs, risk_count.rs, settings, deps.rs, view.rs.
//! Настройки: ID чатов ролей Telegram (notify.telegram_chat, notify.control_chat, notify.failures_chat …, решение 176).

use serde_json::json;

use crate::core::types::{CommandSource, ManualAction, ManualCommand, VenueId};
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::risk::{self, Mode};

impl Core {
    /// Выполняет команду человека. Возвращает: Ok(текст) — выполнено; Err(причина) — отклонено.
    /// Обе ветки пишутся в журнал с источником команды.
    pub async fn command(&self, cmd: ManualCommand) -> Result<String, String> {
        // Решение 110: команда выполняется отдельной задачей и доходит до конца, даже если интерфейс отключился.
        // Живой случай 02.11: «Продать и перегнать» у остановленной сделки ATOM — сделка уже помечена «идёт продажа»,
        // ответа баланса нет, связь с интерфейсом рвётся, веб-сервер бросает запрос — и сделка навсегда остаётся
        // в шаге Selling: пропадает с экрана, а предварительный убыток и сверка балансов её держат.
        let core = self.clone();
        let c = cmd.clone();
        tokio::spawn(async move { core.command_inner(c).await })
            .await
            .unwrap_or_else(|e| Err(format!("команда прервана: {e}")))
    }

    /// Выполнение команды и запись в журнал (см. command).
    async fn command_inner(&self, cmd: ManualCommand) -> Result<String, String> {
        // Решение 177: команда человека — ручная: её сбои не «ошибки площадки подряд», паузу ставит только автоматика
        // (risk_count.rs). Продажи и перегон отдельной задачей ставят тот же признак сами (manual.rs, manual_halted.rs,
        // manual_usdt.rs).
        let res = super::risk_count::manual(self.run_command(&cmd)).await;
        // Решение 156: итог команды — Telegram правит сообщения записи «выполнено: кто, что, итог» (notify/mirror.rs).
        self.inner.live.send(super::view::CoreEvent::Command(json!({
            "action": cmd.action, "who": who(&cmd), "ok": res.is_ok(),
            "text": res.clone().unwrap_or_else(|e| e),
        })));
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
            // Решение 176: чат — один из чатов ролей (notify/roles.rs); какая кнопка из какой роли — проверил notify.
            // Настройка может быть @username чата — тогда совпадение уже проверил notify (roles::chat_matches),
            // у ядра есть только числовой id.
            if !crate::notify::roles::is_role_chat(&self.inner.deps.settings, chat) {
                return Err(format!("чат {chat} — не настроенный чат уведомлений"));
            }
        }
        // Решение 176: «Перевести USDT» (Telegram «Управление» и интерфейс) — usdt_transfer.rs.
        if let Some(r) = super::usdt_transfer::command(self, &cmd.action, &who(cmd)).await {
            return r;
        }
        // Решение 174: «Продать» панели «Балансы» — balance_sell.rs.
        if let Some(r) = super::balance_sell::command(self, &cmd.action, &who(cmd)).await {
            return r;
        }
        // Решение 163: «Перегнать» / «Перегнано вручную» записи «USDT не вернулись» — manual_usdt.rs.
        if let Some(r) = super::manual_usdt::command(self, &cmd.action, &who(cmd)).await {
            return r;
        }
        // Команда по уже закрытому алерту (или вернувшимся монетам) — спокойно игнорируется (Issue #10).
        let gone = match &cmd.action {
            ManualAction::SellNow { alert_id }
            | ManualAction::SellNowReturn { alert_id }
            | ManualAction::TakeOver { alert_id } => (!lk(&self.inner.alerts)
                .contains_key(alert_id))
            .then(|| format!("алерт {alert_id}")),
            ManualAction::ReturnToBot { holding_id } => (!lk(&self.inner.holdings)
                .contains_key(holding_id))
            .then(|| format!("запись {holding_id}")),
            // Шаг сделки проверяют сами команды (manual_halted.rs) — отказ с причиной.
            ManualAction::SellHalted { .. }
            | ManualAction::SellHaltedReturn { .. }
            | ManualAction::SoldManually { .. } => None,
            // Решение 157: команды плашки режима — commands_control.rs.
            _ => return super::commands_control::run(self, &cmd.action, &who(cmd)).await,
        };
        if let Some(what) = gone {
            return Ok(format!(
                "{what} уже закрыт(а) или не существует — команда проигнорирована"
            ));
        }
        let who = who(cmd);
        match &cmd.action {
            ManualAction::SellNow { alert_id } => {
                super::manual::sell_now(self, alert_id, false).await
            }
            ManualAction::SellNowReturn { alert_id } => {
                super::manual::sell_now(self, alert_id, true).await
            }
            ManualAction::TakeOver { alert_id } => {
                super::manual::take_over(self, alert_id, &who).await
            }
            ManualAction::ReturnToBot { holding_id } => {
                super::manual_return::return_to_bot(self, holding_id).await
            }
            ManualAction::SellHalted { deal_id } => {
                super::manual_halted::sell_halted(self, deal_id, false).await
            }
            ManualAction::SellHaltedReturn { deal_id } => {
                super::manual_halted::sell_halted(self, deal_id, true).await
            }
            ManualAction::SoldManually { deal_id, usdt } => {
                super::manual_halted::sold_manually(self, deal_id, usdt, &who).await
            }
            _ => super::commands_control::run(self, &cmd.action, &who).await,
        }
    }

    /// Экран «Управление»: режим системы (Running — «Старт», Stopped — «Стоп», Paused — «Пауза»).
    /// Возвращает: текст для журнала. Ошибок нет.
    pub async fn set_mode(&self, mode: Mode) -> String {
        self.set_mode_by(mode, "команда из интерфейса").await
    }

    /// То же, что set_mode, с причиной «кто» (решение 157: Telegram — «команда из Telegram, @имя»). Ошибок нет.
    pub async fn set_mode_by(&self, mode: Mode, reason: &str) -> String {
        let t = risk::set_mode(self, mode, Some(reason.into()));
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

    /// Снять карантин монеты (ручная проверка после «депозит не зачислен» или медленного перевода). Принимает: id из
    /// интерфейса — адрес контракта (0x…) или тикер монеты без адреса (решение 118, quarantine.rs). Ошибок нет.
    pub async fn release_quarantine(&self, id: &str) {
        let label = super::quarantine::release_id(self, id)
            .map(|q| q.label)
            .unwrap_or_else(|| id.to_string());
        let t = format!("{label}: карантин снят вручную");
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

/// Кто дал команду — для журнала и итога: «интерфейс» или «Telegram <чат>, <пользователь>».
pub fn who(cmd: &ManualCommand) -> String {
    match (&cmd.source, &cmd.by) {
        (CommandSource::Ui, _) => "интерфейс".to_string(),
        (CommandSource::Telegram { chat }, Some(by)) => format!("Telegram {chat}, {by}"),
        (CommandSource::Telegram { chat }, None) => format!("Telegram {chat}"),
    }
}
