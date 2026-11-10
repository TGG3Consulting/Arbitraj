//! Файл: notify/mirror_send.rs
//! Что делает: часть дубля «Ручного управления» (решение 156, продолжение mirror.rs — размер файла): напоминание
//! пачкой (одно сообщение со списком ждущих записей и их кнопками, у прежнего напоминания кнопки снимаются,
//! editMessageReplyMarkup — https://core.telegram.org/bots/api#editmessagereplymarkup), итог команды
//! «Выполнено: кто, что, итог» в сообщениях ушедших записей, ошибки Telegram (429 — пауза retry_after) и журнал.
//! Решение 177: правка сообщения сменившейся записи (update_changed) и итог продажи кнопкой (sale_final).
//! Решение 251: тексты — шаблон алерта в HTML (alert.rs::telegram, tg_html.rs);; полный текст причины — в журнал при первой отправке записи (log_full).
//! Кто вызывает: notify/mirror.rs.
//! От чего зависит: notify/mirror_state.rs, mirror_track.rs, mirror_view.rs, tg_methods.rs, tg_html.rs, alert.rs,
//! client.rs, settings, storage/journal.
//! Настройки: notify.manual_repeat_s (через Mirror::every), notify.updates_retry_ms (пауза после ошибки Telegram).

use std::time::{Duration, Instant};

use crate::core::types::{ManualAction, ManualRecord};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::alert::telegram;
use super::mirror::Mirror;
use super::mirror_state::lock;
use super::mirror_track::{final_text, is_sale};
use super::mirror_view::{batch_message_for, closed_text, record_message_for, reminder_header};
use super::telegram::{Telegram, TgError};
use super::{client, tg_methods};

impl Mirror {
    /// Напоминание пачкой, если пора. Принимает: клиента, (чат, показывать ли недоступные кнопки), записи, разрешённые.
    pub(super) async fn remind(
        &self,
        tg: &Telegram,
        (chat, all): (&str, bool),
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
        let (text, kb) = batch_message_for(&list, &reminder_header(list.len()), all);
        match tg.send_html(chat, &telegram(&text), kb).await {
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

    /// Решение 177: сменившиеся и вернувшиеся записи (Diff.changed) — правка текста и кнопок первого сообщения записи.
    /// Принимает: клиента, (чат, показывать ли недоступные кнопки), записи. Ошибки — в журнал.
    pub(super) async fn update_changed(
        &self,
        tg: &Telegram,
        (chat, all): (&str, bool),
        recs: &[ManualRecord],
    ) {
        for r in recs {
            let Some(id) = lock(&self.state)
                .sent
                .get(&r.key)
                .and_then(|s| s.msgs.first().copied())
            else {
                continue;
            };
            let (text, kb) = record_message_for(r, all);
            match tg.edit_html(chat, id, &telegram(&text), kb).await {
                Ok(()) => lock(&self.state).set_text(&r.key, text),
                Err(e) => {
                    self.log(
                        Level::Warn,
                        format!("Ручное управление: сообщение {id} не обновлено: {e}"),
                    )
                    .await
                }
            }
        }
    }

    /// Итог команды ({action, who, ok, text} из Core::command): сообщения ушедших записей — «Выполнено: …».
    /// Решение 177: итог продажи кнопкой ({final: true, deal, ok, text}, core/manual_outcome.rs) — ещё одна правка тех же
    /// сообщений («Выполнено: продано …» / «Не продано: …»); запись вернулась на экран — её уже правит update_changed.
    pub(super) async fn outcome(&self, v: &serde_json::Value) {
        if v["final"].as_bool() == Some(true) {
            return self.sale_final(v).await;
        }
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
        let role = super::roles::mirror_role(&self.notifier.settings);
        let Ok(ready) = client::build_role(
            &self.notifier.http,
            &self.notifier.settings,
            &self.notifier.vault,
            role,
        ) else {
            return;
        };
        let who = v["who"].as_str().unwrap_or_default();
        let res = v["text"].as_str().unwrap_or_default();
        let o = format!("{who}: «{}» — {res}", action.title());
        let sale = is_sale(&action);
        for (rec, mut sent) in closed {
            let text = closed_text(&sent.text, Some(&o));
            let html = telegram(&text);
            for id in &sent.msgs {
                let _ = ready.tg.edit_html(&ready.chat, *id, &html, None).await;
            }
            if sale {
                sent.text = text;
                lock(&self.state).keep_finished(rec, sent);
            }
        }
    }

    /// Итог продажи кнопкой: сообщения записей сделки, ждавших его, — «… \n\nВыполнено: продано …» / «Не продано: …».
    async fn sale_final(&self, v: &serde_json::Value) {
        let (Some(deal), Some(res)) = (v["deal"].as_str(), v["text"].as_str()) else {
            return;
        };
        let list = lock(&self.state).take_finished(deal);
        if list.is_empty() {
            return;
        }
        let role = super::roles::mirror_role(&self.notifier.settings);
        let Ok(ready) = client::build_role(
            &self.notifier.http,
            &self.notifier.settings,
            &self.notifier.vault,
            role,
        ) else {
            return;
        };
        for (_, sent) in list {
            let html = telegram(&final_text(&sent.text, res));
            for id in &sent.msgs {
                let _ = ready.tg.edit_html(&ready.chat, *id, &html, None).await;
            }
        }
    }

    /// Решение 251: полный текст причины записи — в журнал при первой отправке (в сообщении причина коротко, хвост
    /// «Подробно (полностью — в журнале)»). Принимает: запись. Ошибка журнала уже выведена в лог процесса.
    pub(super) async fn log_full(&self, r: &ManualRecord) {
        let _ = self
            .notifier
            .journal
            .write(
                Level::Info,
                "notify",
                Some(&r.venue),
                format!("Ручное управление в Telegram: запись {}", r.key),
                Some(serde_json::json!({ "deal": r.deal_id, "reason_full": r.reason_full })),
            )
            .await;
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
