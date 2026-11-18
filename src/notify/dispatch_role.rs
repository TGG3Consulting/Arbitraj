//! Файл: notify/dispatch_role.rs
//! Что делает: отправка события ещё и в чаты ролей (решение 176) по таблице notify/routes.rs: «Общий» — как раньше
//! (Notifier::send, фильтры notify.event.*), роли из таблицы — если их чат задан. Отправка в роль — отдельной задачей:
//! ядро не ждёт Telegram (закрытие сделки не задерживается). Ответ 429 — одна повторная попытка ровно через
//! parameters.retry_after (ResponseParameters, https://core.telegram.org/bots/api#responseparameters).
//! Решение 340: send_tagged не ждёт и «Общего» — задание в очередь уведомлений (notify/queue.rs); в задании
//! (tagged_now) — роли своими задачами со снимком карточки (alert_card::current), «Общий» — по порядку очереди.
//! Решение 408 (t193): кнопки события (notify/acts.rs) — и в чатах ролей.
//! Кто вызывает: core/deps.rs (notify_tagged) — закрытие сделки (core/ledger.rs); core/deal_closed.rs (tagged_now в
//! задании Notifier::later).
//! От чего зависит: notify/dispatch.rs (html — шаблон решения 251, log, deliver), routes.rs, roles.rs, client.rs,
//! catalog.rs, tg_methods.rs, tg_html.rs, queue.rs, alert_card.rs, glue.rs.
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
    /// Решение 340: не ждёт Telegram — задание в очередь (notify/queue.rs), возврат сразу.
    pub async fn send_tagged(&self, code: &str, venue: Option<&str>, text: &str, tag: Tag) {
        let snap = super::queue::snap(code, text);
        let (n, c, v, t) = (
            self.clone(),
            code.to_string(),
            venue.map(str::to_string),
            text.to_string(),
        );
        let job = async move { n.tagged_now(&c, v.as_deref(), &t, tag).await };
        super::queue::push(self, code, venue, snap, Box::pin(job));
    }

    /// Отправка send_tagged внутри задания очереди: роли — своими задачами (со снимком карточки задания), «Общий» —
    /// здесь (склеиваемые события — glue.rs, как в send). Принимает: как send_tagged. Ошибок нет: всё — в журнал.
    pub async fn tagged_now(&self, code: &str, venue: Option<&str>, text: &str, tag: Tag) {
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
            let job = async move { n.to_role(role, &code, venue.as_deref(), &text).await };
            // Решение 408: кнопки события (notify/acts.rs) — и в чат роли.
            let job = super::acts::scoped(super::acts::current(), job);
            tokio::spawn(super::alert_card::scoped(super::alert_card::current(), job));
        }
        if super::glue::glued(code) {
            return super::glue::push(self, code, venue, text);
        }
        self.deliver(code, venue, text, None).await;
    }

    /// Одно сообщение в чат роли (решение 408: с кнопками события, если они есть — notify/acts.rs). Принимает: роль,
    /// код, площадку, текст. Ошибок нет: итог — в журнал.
    async fn to_role(&self, role: Role, code: &str, venue: Option<&str>, text: &str) {
        let Some(def) = catalog::find(code) else {
            return;
        };
        // Решение 251: тот же шаблон алерта в HTML, что у «Общего» (notify/alert.rs).
        // Решение 408: кнопки события (notify/acts.rs); не влезшие в 64 байта — строкой в тексте.
        let (kb, notes) = super::acts::keyboard(&super::acts::current());
        let msg = if notes.is_empty() {
            html(code, def, text)
        } else {
            html(code, def, &format!("{text}\n\n{}", notes.join("\n")))
        };
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
        let mut res = ready.tg.send_html(&ready.chat, &msg, kb.clone()).await;
        if let Some(s) = res.as_ref().err().and_then(tg_methods::retry_after) {
            tokio::time::sleep(Duration::from_secs(s)).await;
            res = ready.tg.send_html(&ready.chat, &msg, kb).await;
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
