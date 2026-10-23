//! Файл: notify/dispatch.rs
//! Что делает: решает, отправлять ли событие в Telegram, и отправляет.
//! Событие уходит, только если в интерфейсе включено (notify.event.enabled = да)
//! и площадка события есть в списке notify.event.venues (пустой список или не задан — все площадки).
//! Результат отправки (или причина, почему не ушло) пишется в журнал.
//! Кто вызывает: любые компоненты через Notifier::send (и send_with_buttons — для событий с кнопками,
//! например stuck.loss_alert); notify/settings_watch.rs.
//! От чего зависит: notify/client.rs, notify/catalog.rs, notify/callback.rs, notify/tg_buttons.rs,
//! settings, secrets, storage/journal.
//! Настройки: notify.event.enabled / venues / condition (область «событие»), notify.telegram_*.
//!
//! notify.event.condition пока не проверяется: формат условия утверждается на этапе 5.
//! Если условие задано, событие НЕ отправляется, а в журнал пишется причина — чтобы не слать
//! того, что Тигран хотел отфильтровать.

use serde_json::json;

use crate::secrets::Vault;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::{callback, catalog, client};

/// Отправитель уведомлений. Дешёво клонируется.
#[derive(Clone)]
pub struct Notifier {
    pub http: reqwest::Client,
    pub settings: Store,
    pub vault: Vault,
    pub journal: Journal,
}

impl Notifier {
    /// Отправляет событие, если оно включено.
    /// Принимает: код события из notify/catalog.rs, площадку (если есть), текст.
    /// Ошибок не возвращает: всё, что пошло не так, пишется в журнал.
    pub async fn send(&self, code: &str, venue: Option<&str>, text: &str) {
        self.deliver(code, venue, text, None).await
    }

    /// Отправляет событие с кнопками из каталога (решение 37), если оно включено.
    /// Принимает: код события, площадку, текст, id объекта, к которому относятся кнопки (например id алерта
    /// StuckAlert) — он уходит в callback_data. Если кнопки собрать нельзя (id длиннее предела протокола),
    /// сообщение всё равно уходит — без кнопок, с причиной в тексте и в журнале.
    /// Ошибок не возвращает: всё, что пошло не так, пишется в журнал.
    pub async fn send_with_buttons(
        &self,
        code: &str,
        venue: Option<&str>,
        text: &str,
        object_id: &str,
    ) {
        self.deliver(code, venue, text, Some(object_id)).await
    }

    /// Общая часть send и send_with_buttons: фильтры события, сборка клиента, отправка, журнал.
    async fn deliver(&self, code: &str, venue: Option<&str>, text: &str, object: Option<&str>) {
        let Some(def) = catalog::find(code) else {
            self.log(
                Level::Error,
                venue,
                format!("Неизвестное событие уведомлений: {code}"),
            )
            .await;
            return;
        };
        let enabled = self
            .settings
            .event_value("notify.event.enabled", code)
            .and_then(|v| v.as_bool());
        if enabled != Some(true) {
            return;
        }
        if let (Some(v), Some(list)) = (
            venue,
            self.settings.event_value("notify.event.venues", code),
        ) {
            let allowed: Vec<&str> = list
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            if !allowed.is_empty() && !allowed.contains(&v) {
                return;
            }
        }
        if self
            .settings
            .event_value("notify.event.condition", code)
            .is_some()
        {
            self.log(
                Level::Warn,
                venue,
                format!(
                    "«{}» не отправлено: условия событий проверяются с этапа 5",
                    def.title
                ),
            )
            .await;
            return;
        }
        let ready = match client::build(&self.http, &self.settings, &self.vault) {
            Ok(r) => r,
            Err(e) => {
                self.log(
                    Level::Warn,
                    venue,
                    format!("«{}» не отправлено: {}", def.title, e.message),
                )
                .await;
                return;
            }
        };
        let mut body = format!("{}\n{}", def.title, text);
        let kb = match object.map(|id| callback::keyboard(def, id)) {
            None | Some(Ok(None)) => None,
            Some(Ok(Some(kb))) => Some(kb),
            Some(Err(e)) => {
                let why = format!("кнопки не добавлены: {e}");
                self.log(Level::Error, venue, format!("«{}»: {why}", def.title))
                    .await;
                body.push_str(&format!("\n\n({why})"));
                None
            }
        };
        let sent = match kb {
            Some(kb) => ready.tg.send_with_keyboard(&ready.chat, &body, kb).await,
            None => ready.tg.send(&ready.chat, &body).await,
        };
        match sent {
            Ok(()) => {
                self.log(
                    Level::Info,
                    venue,
                    format!("Отправлено в Telegram: {}", def.title),
                )
                .await
            }
            Err(e) => {
                self.log(
                    Level::Error,
                    venue,
                    format!("«{}» не отправлено: {e}", def.title),
                )
                .await
            }
        }
    }

    /// Запись в журнал от имени уведомлений. Ошибка журнала уже выведена в лог процесса.
    async fn log(&self, level: Level, venue: Option<&str>, message: String) {
        let _ = self
            .journal
            .write(level, "notify", venue, message, Some(json!({})))
            .await;
    }
}
