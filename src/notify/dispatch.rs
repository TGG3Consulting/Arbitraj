//! Файл: notify/dispatch.rs
//! Что делает: решает, отправлять ли событие в Telegram, и отправляет.
//! Событие уходит, только если в интерфейсе включено (notify.event.enabled = да)
//! и площадка события есть в списке notify.event.venues (пустой список или не задан — все площадки).
//! Результат отправки (или причина, почему не ушло) пишется в журнал.
//! Кто вызывает: любые компоненты через Notifier::send (и send_with_buttons — для событий с кнопками,
//! например stuck.loss_alert; send_tagged — событие ещё и в чаты ролей, dispatch_role.rs); notify/settings_watch.rs.
//! Это чат «Общий» (решение 176, notify/roles.rs): его поведение не меняется.
//! Решение 251: текст — по единому шаблону (notify/alert.rs) в HTML (notify/tg_html.rs); исходный текст события —
//! в журнал (data.text записи «Отправлено в Telegram»).
//! Решение 251 (г, Тигран 10.11): deal.closed и stuck.loss_alert не идут в «Общий», если задан чат их роли
//! (notify/routes.rs NOT_GENERAL); события связи площадки склеиваются по площадке (notify/glue.rs).
//! Решение 251, дополнение (Тигран 10.11): выключенное событие (notify.event.enabled = нет) — одной записью в журнал,
//! если у места события своей записи нет (notify/journal_off.rs).
//! От чего зависит: notify/client.rs, notify/catalog.rs, notify/callback.rs, notify/alert.rs, notify/tg_html.rs,
//! notify/routes.rs, notify/glue.rs, notify/journal_off.rs,
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

/// Текст сообщения события (без разметки). Решение 164: первая строка — значок и что случилось (notify/heads.rs);
/// текст со своим заголовком (собран notify/human.rs Msg) уходит как есть; текст ядра «<id сделки>: …» — по шаблону
/// решения 251 (Msg): «Сделка: <id>» под заголовком, строки текста — фактами (длинный hex и JSON сокращены, ссылки на
/// номера решений убраны). Принимает: код, событие каталога, текст. Возвращает: текст. Ошибок нет.
pub fn body(code: &str, def: &catalog::EventDef, text: &str) -> String {
    if super::human::has_head(text) {
        return text.to_string();
    }
    let (id, rest) = super::human::split_deal(text);
    let mut m = super::human::Msg::titled(code, def.title);
    for l in super::reason::tidy(rest).lines() {
        m = m.line(l);
    }
    m.id(id.unwrap_or_default()).render()
}

/// Текст события для Telegram: HTML по шаблону решения 251 (notify/alert.rs::telegram). Ошибок нет.
pub fn html(code: &str, def: &catalog::EventDef, text: &str) -> String {
    super::alert::telegram(&body(code, def, text))
}

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
    /// Решение 251 (г): события связи площадки (notify/glue.rs GLUED) склеиваются — одно сообщение на площадку за пачку.
    pub async fn send(&self, code: &str, venue: Option<&str>, text: &str) {
        if super::glue::glued(code) {
            return super::glue::push(self, code, venue, text);
        }
        self.deliver(code, venue, text, None).await;
    }

    /// Как send, но ответ 429 — одна повторная попытка ровно через parameters.retry_after (ResponseParameters,
    /// https://core.telegram.org/bots/api#responseparameters). Решение 251: склейка изменений настроек
    /// (notify/settings_watch.rs) — 09.11 19:12 пачка изменений получила 429. Ошибок нет: всё — в журнал.
    pub async fn send_retry(&self, code: &str, venue: Option<&str>, text: &str) {
        if let Some(s) = self.deliver(code, venue, text, None).await {
            tokio::time::sleep(std::time::Duration::from_secs(s)).await;
            self.deliver(code, venue, text, None).await;
        }
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
        self.deliver(code, venue, text, Some(object_id)).await;
    }

    /// Общая часть send и send_with_buttons: фильтры события, сборка клиента, отправка, журнал.
    /// Возвращает: Some(секунды) — Telegram ответил 429 с retry_after (повторить можно через столько), иначе None.
    pub(super) async fn deliver(
        &self,
        code: &str,
        venue: Option<&str>,
        text: &str,
        object: Option<&str>,
    ) -> Option<u64> {
        let Some(def) = catalog::find(code) else {
            self.log(
                Level::Error,
                venue,
                format!("Неизвестное событие уведомлений: {code}"),
            )
            .await;
            return None;
        };
        // Решение 251, дополнение (Тигран 10.11): выключенное событие — одной записью в журнал (notify/journal_off.rs).
        if super::journal_off::off(&self.settings, code) {
            self.journal_off(code, def, venue, text).await;
            return None;
        }
        if !self.passes(code, venue) {
            return None;
        }
        // Решение 251 (г): событие идёт в чат своей роли вместо «Общего», если он задан (routes.rs NOT_GENERAL).
        if super::routes::not_general(&self.settings, code).is_some() {
            return None;
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
            return None;
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
                return None;
            }
        };
        let mut plain = body(code, def, text);
        let kb = match object.map(|id| callback::keyboard(def, id)) {
            None | Some(Ok(None)) => None,
            Some(Ok(Some(kb))) => Some(kb),
            Some(Err(e)) => {
                let why = format!("кнопки не добавлены: {e}");
                self.log(Level::Error, venue, format!("«{}»: {why}", def.title))
                    .await;
                plain.push_str(&format!("\n\n({why})"));
                None
            }
        };
        // Решение 251: шаблон алерта в HTML (notify/alert.rs); полный исходный текст — в журнал (data.text), туда
        // же ведёт строка «Подробно (полностью — в журнале)».
        let sent = ready
            .tg
            .send_html(&ready.chat, &super::alert::telegram(&plain), kb)
            .await;
        match sent {
            Ok(_) => {
                let _ = self
                    .journal
                    .write(
                        Level::Info,
                        "notify",
                        venue,
                        format!("Отправлено в Telegram: {}", def.title),
                        Some(json!({ "code": code, "text": text })),
                    )
                    .await;
                None
            }
            Err(e) => {
                self.log(
                    Level::Error,
                    venue,
                    format!("«{}» не отправлено: {e}", def.title),
                )
                .await;
                super::tg_methods::retry_after(&e)
            }
        }
    }

    /// Включено ли событие (notify.event.enabled = да) и разрешена ли площадка (notify.event.venues).
    /// Принимает: код события, площадку. Возвращает: true — проходит. Ошибок нет.
    fn passes(&self, code: &str, venue: Option<&str>) -> bool {
        let enabled = self
            .settings
            .event_value("notify.event.enabled", code)
            .and_then(|v| v.as_bool());
        if enabled != Some(true) {
            return false;
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
                return false;
            }
        }
        true
    }

    /// Уйдёт ли событие (как в deliver: включено, площадка, условие не задано) — для дубля «Ручного управления»
    /// (notify/mirror.rs, событие manual.record), который шлёт сам. Принимает: код, площадку. Ошибок нет.
    pub fn allows(&self, code: &str, venue: Option<&str>) -> bool {
        self.passes(code, venue)
            && self
                .settings
                .event_value("notify.event.condition", code)
                .is_none()
    }

    /// Запись в журнал от имени уведомлений. Ошибка журнала уже выведена в лог процесса.
    pub(super) async fn log(&self, level: Level, venue: Option<&str>, message: String) {
        let _ = self
            .journal
            .write(level, "notify", venue, message, Some(json!({})))
            .await;
    }
}
