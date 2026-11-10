//! Файл: notify/dispatch_role.rs
//! Что делает: отправка события ещё и в чаты ролей (решение 176) по таблице notify/routes.rs: «Общий» — как раньше
//! (Notifier::send, фильтры notify.event.*), роли из таблицы — если их чат задан. Отправка в роль — отдельной задачей:
//! ядро не ждёт Telegram (закрытие сделки не задерживается). Ответ 429 — одна повторная попытка ровно через
//! parameters.retry_after (ResponseParameters, https://core.telegram.org/bots/api#responseparameters).
//! Кто вызывает: core/deps.rs (notify_tagged) — закрытие сделки (core/ledger.rs).
//! От чего зависит: notify/dispatch.rs (html — шаблон решения 251, log), routes.rs, roles.rs, client.rs, catalog.rs,
//! tg_methods.rs, tg_html.rs.
//! Настройки: ID чатов и токены ролей (notify/roles.rs), notify.telegram_api_url, http.timeout_ms.

use std::time::Duration;

use crate::storage::journal::Level;

use super::dispatch::{Notifier, html};
use super::roles::{self, Role};
use super::routes::{self, Tag};
use super::{catalog, client, tg_methods};

impl Notifier {
    /// Событие в «Общий» (как send) и в чаты ролей по таблице маршрутов.
    /// Принимает: код события, площадку, текст, признак (знак итога сделки). Ошибок нет: всё — в журнал.
    pub async fn send_tagged(&self, code: &str, venue: Option<&str>, text: &str, tag: Tag) {
        for role in routes::extra(code, tag) {
            if roles::chat(&self.settings, *role).is_none() {
                continue;
            }
            let (n, role, code, venue, text) = (
                self.clone(),
                *role,
                code.to_string(),
                venue.map(str::to_string),
                text.to_string(),
            );
            tokio::spawn(async move { n.to_role(role, &code, venue.as_deref(), &text).await });
        }
        self.send(code, venue, text).await;
    }

    /// Одно сообщение в чат роли (без кнопок). Принимает: роль, код, площадку, текст. Ошибок нет: итог — в журнал.
    async fn to_role(&self, role: Role, code: &str, venue: Option<&str>, text: &str) {
        let Some(def) = catalog::find(code) else {
            return;
        };
        // Решение 251: тот же шаблон алерта в HTML, что у «Общего» (notify/alert.rs).
        let msg = html(code, def, text);
        let ready = match client::build_role(&self.http, &self.settings, &self.vault, role) {
            Ok(r) => r,
            Err(e) => {
                let why = format!(
                    "«{}» в чат «{}» не отправлено: {}",
                    def.title,
                    role.title(),
                    e.message
                );
                return self.log(Level::Warn, venue, why).await;
            }
        };
        let mut res = ready.tg.send_html(&ready.chat, &msg, None).await;
        if let Some(s) = res.as_ref().err().and_then(tg_methods::retry_after) {
            tokio::time::sleep(Duration::from_secs(s)).await;
            res = ready.tg.send_html(&ready.chat, &msg, None).await;
        }
        let (level, line) = match res {
            Ok(_) => (
                Level::Info,
                format!(
                    "Отправлено в Telegram, чат «{}»: {}",
                    role.title(),
                    def.title
                ),
            ),
            Err(e) => (
                Level::Error,
                format!(
                    "«{}» в чат «{}» не отправлено: {e}",
                    def.title,
                    role.title()
                ),
            ),
        };
        self.log(level, venue, line).await;
    }
}
