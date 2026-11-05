//! Файл: notify/press_record.rs
//! Что делает: нажатие кнопки записи «Ручного управления» в Telegram (решение 156) — по ТЕКУЩИМ записям экрана
//! (Core::manual_records): записи нет на экране или она уже выполнена из Telegram — «Уже выполнено», действие
//! не повторяется; кнопка недоступна — ответ той же причиной, что на экране; кнопка с вводом («Продано вручную») —
//! вопрос суммы с ForceReply (ответ человека — notify/tg_commands.rs); иначе — подтверждение «Да, …» (как confirm
//! на экране, press_confirm.rs). Старые сообщения до решения 156 («sell_now:<id алерта>») — как запись алерта.
//! Кто вызывает: notify/press.rs.
//! От чего зависит: core (manual_records), core/types/manual_feed.rs, mirror_state.rs, press_confirm.rs, tg_methods.rs.
//! Настройки: нет.
//!
//! ForceReply: https://core.telegram.org/bots/api#forcereply

use crate::core::types::{ManualRecord, RecordKind};
use crate::storage::journal::Level;

use super::mirror_state::{Pending, Prompt, lock};
use super::press::Ctx;
use super::tg_buttons::{CallbackQuery, CbMessage};
use super::tg_methods::force_reply;

/// Ответ на кнопку записи, которой уже нет на экране (или выполненной из Telegram).
pub const ALREADY: &str = "Уже выполнено — записи нет на экране «Ручное управление»";

/// Ключ записи из callback_data: «a.<id>» и т.п.; без вида (сообщения до решения 156) — алерт.
pub fn record_key(object: &str) -> String {
    match object.split_once('.') {
        Some((k, _)) if k.len() == 1 => object.to_string(),
        _ => ManualRecord::key_of(RecordKind::Alert, object),
    }
}

impl Ctx<'_> {
    /// Текущая запись экрана по ключу, если она ещё ждёт действия. Возвращает: запись или None («уже выполнено»).
    pub fn live_record(&self, key: &str) -> Option<ManualRecord> {
        if lock(self.state).done.contains(key) {
            return None;
        }
        self.core
            .manual_records()
            .into_iter()
            .find(|r| r.key == key)
    }

    /// Кнопка записи. Принимает: нажатие, сообщение, код кнопки, объект из callback_data. Ошибок нет.
    pub async fn record_button(
        &self,
        q: &CallbackQuery,
        msg: &CbMessage,
        code: &str,
        object: &str,
    ) {
        let key = record_key(object);
        let Some(rec) = self.live_record(&key) else {
            return self.answer(&q.id, Some(ALREADY)).await;
        };
        let Some(b) = rec.button(code).cloned() else {
            return self.answer(&q.id, Some("Неизвестная кнопка")).await;
        };
        if let Some(why) = &b.disabled {
            return self
                .answer(&q.id, Some(&format!("«{}» недоступна: {why}", b.title)))
                .await;
        }
        let who = q.from.describe();
        if let Some(question) = &b.input {
            let text = format!("{question}\nОтветьте на это сообщение числом (USDT).");
            match self
                .tg
                .send_markup(
                    self.chat,
                    &text,
                    Some(force_reply("USDT")),
                    Some(msg.message_id),
                )
                .await
            {
                Ok(id) => {
                    lock(self.state).prompts.insert(
                        (msg.chat.id, id),
                        Prompt {
                            key,
                            code: code.into(),
                        },
                    );
                    self.answer(&q.id, Some("Введите сумму ответом на сообщение бота"))
                        .await;
                }
                Err(e) => {
                    self.answer(&q.id, Some("Не удалось спросить сумму — смотрите журнал"))
                        .await;
                    self.log(
                        Level::Error,
                        format!("«{}»: вопрос суммы не отправлен: {e}", b.title),
                    )
                    .await;
                }
            }
            return;
        }
        self.log(
            Level::Info,
            format!(
                "Telegram: {who} нажал «{}» ({key}) — ждём подтверждения",
                b.title
            ),
        )
        .await;
        let p = Pending {
            key: Some(key),
            action: b.action.clone(),
            title: b.title.clone(),
            question: b.confirm.clone(),
        };
        self.ask(&q.id, Some(msg.message_id), p).await;
    }
}
