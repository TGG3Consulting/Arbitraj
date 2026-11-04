//! Файл: notify/mirror_send.rs
//! Что делает: часть дубля «Ручного управления» (решение 156, продолжение mirror.rs — размер файла): напоминание
//! пачкой (одно сообщение со списком ждущих записей и их кнопками, у прежнего напоминания кнопки снимаются,
//! editMessageReplyMarkup — https://core.telegram.org/bots/api#editmessagereplymarkup), итог команды
//! «Выполнено: кто, что, итог» в сообщениях ушедших записей, ошибки Telegram (429 — пауза retry_after) и журнал.
//! Кто вызывает: notify/mirror.rs.
//! От чего зависит: notify/mirror_state.rs, mirror_view.rs, tg_methods.rs, client.rs, settings, storage/journal.
//! Настройки: notify.manual_repeat_s (через Mirror::every), notify.updates_retry_ms (пауза после ошибки Telegram).

use std::time::{Duration, Instant};

use crate::core::types::{ManualAction, ManualRecord};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::mirror::Mirror;
use super::mirror_state::lock;
use super::mirror_view::{batch_message, closed_text, reminder_header};
use super::telegram::{Telegram, TgError};
use super::{client, tg_methods};

impl Mirror {
    /// Напоминание пачкой, если пора. Принимает: клиента, чат, записи, разрешённые записи.
    pub(super) async fn remind(
        &self,
        tg: &Telegram,
        chat: &str,
        recs: &[ManualRecord],
        allowed: &[&ManualRecord],
    ) {
        let now = Instant::now();
        let list: Vec<&ManualRecord> = {
            let st = lock(&self.state);
            if !st.reminder_due(now, self.every()) {
                return;
            }
            st.waiting(recs)
                .into_iter()
                .filter(|r| st.sent.contains_key(&r.key) && allowed.iter().any(|a| a.key == r.key))
                .collect()
        };
        if list.is_empty() {
            // Напоминать некого (событие выключено, площадка не в списке) — отсчёт не идёт.
            lock(&self.state).last_reminder = None;
            return;
        }
        let (text, kb) = batch_message(&list, &reminder_header(list.len()));
        match tg.send_markup(chat, &text, kb, None).await {
            Ok(id) => {
                let old = {
                    let mut st = lock(&self.state);
                    st.last_reminder = Some(Instant::now());
                    st.reminder.replace(id)
                };
                if let Some(old) = old
                    && let Err(e) = tg.edit_markup(chat, old, None).await
                {
                    self.log(
                        Level::Warn,
                        format!("Напоминание {old}: кнопки не сняты: {e}"),
                    )
                    .await;
                }
            }
            Err(e) => {
                // Следующая попытка — через интервал (или после паузы retry_after).
                lock(&self.state).last_reminder = Some(Instant::now());
                self.tg_error(e, "напоминание").await
            }
        }
    }

    /// Итог команды ({action, who, ok, text} из Core::command): сообщения ушедших записей — «Выполнено: …».
    pub(super) async fn outcome(&self, v: &serde_json::Value) {
        let Ok(action) = serde_json::from_value::<ManualAction>(v["action"].clone()) else {
            return;
        };
        if v["ok"].as_bool() != Some(true) {
            return;
        }
        let closed = lock(&self.state).take_closed(&action);
        if closed.is_empty() {
            return;
        }
        let Ok(ready) = client::build(
            &self.notifier.http,
            &self.notifier.settings,
            &self.notifier.vault,
        ) else {
            return;
        };
        let who = v["who"].as_str().unwrap_or_default();
        let res = v["text"].as_str().unwrap_or_default();
        let o = format!("{who}: «{}» — {res}", action.title());
        for (_, sent) in closed {
            for id in &sent.msgs {
                let _ = ready
                    .tg
                    .edit_text_markup(&ready.chat, *id, &closed_text(&sent.text, Some(&o)), None)
                    .await;
            }
        }
    }

    /// Ошибка Telegram: 429 — пауза retry_after; иначе — пауза notify.updates_retry_ms (если задана). В журнал.
    pub(super) async fn tg_error(&self, e: TgError, what: &str) {
        let pause = tg_methods::retry_after(&e)
            .map(Duration::from_secs)
            .or_else(|| {
                let s = &self.notifier.settings;
                s.integer("notify.updates_retry_ms", At::default())
                    .ok()
                    .map(Duration::from_millis)
            });
        if let Some(p) = pause {
            lock(&self.state).pause_until = Some(Instant::now() + p);
        }
        self.fail(format!("Ручное управление: {what} не отправлено: {e}"))
            .await;
    }

    /// Ошибка в журнал — одна и та же один раз подряд.
    pub(super) async fn fail(&self, text: String) {
        let again = lock(&self.state).last_error.replace(text.clone()) == Some(text.clone());
        if !again {
            self.log(Level::Error, text).await;
        }
    }

    /// Запись в журнал от имени уведомлений.
    pub(super) async fn log(&self, level: Level, message: String) {
        let _ = self
            .notifier
            .journal
            .write(level, "notify", None, message, None)
            .await;
    }
}
