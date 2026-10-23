//! Файл: notify/tg_buttons.rs
//! Что делает: обращения к Telegram Bot API для кнопок под сообщениями — отправка сообщения
//! с inline-кнопками, приём нажатий длинным опросом getUpdates, ответ на нажатие
//! (answerCallbackQuery) и правка сообщения после действия (editMessageText).
//! Кто вызывает: notify/dispatch.rs (сообщение с кнопками), notify/updates.rs и notify/press.rs (нажатия).
//! От чего зависит: notify/telegram.rs (call_with), notify/tg_text.rs, serde.
//! Настройки: таймауты передаёт вызывающий (notify.updates_timeout_s, http.timeout_ms).
//!
//! Официальная документация: https://core.telegram.org/bots/api
//! - getUpdates: https://core.telegram.org/bots/api#getupdates
//! - Update: https://core.telegram.org/bots/api#update
//! - CallbackQuery: https://core.telegram.org/bots/api#callbackquery
//! - MaybeInaccessibleMessage / InaccessibleMessage: https://core.telegram.org/bots/api#maybeinaccessiblemessage
//! - InlineKeyboardMarkup / InlineKeyboardButton: https://core.telegram.org/bots/api#inlinekeyboardmarkup
//! - answerCallbackQuery: https://core.telegram.org/bots/api#answercallbackquery
//! - editMessageText: https://core.telegram.org/bots/api#editmessagetext
//! - sendMessage (reply_markup): https://core.telegram.org/bots/api#sendmessage

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::telegram::{Telegram, TgError};
use super::tg_text::fit_text;

/// Тип обновлений, которые нужны системе, — только нажатия кнопок.
/// Имя поля Update «callback_query», https://core.telegram.org/bots/api#update
/// (параметр allowed_updates, https://core.telegram.org/bots/api#getupdates).
pub const ALLOWED_UPDATES: &[&str] = &["callback_query"];

/// Update: https://core.telegram.org/bots/api#update — нужны update_id и callback_query.
#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    /// «The update's unique identifier» — по нему считается offset.
    pub update_id: i64,
    pub callback_query: Option<CallbackQuery>,
}

/// CallbackQuery: https://core.telegram.org/bots/api#callbackquery
#[derive(Debug, Clone, Deserialize)]
pub struct CallbackQuery {
    /// Идентификатор нажатия — нужен answerCallbackQuery.
    pub id: String,
    /// Кто нажал.
    pub from: User,
    /// Сообщение бота с кнопкой (MaybeInaccessibleMessage). Нет — кнопка из inline-режима.
    pub message: Option<CbMessage>,
    /// callback_data нажатой кнопки.
    pub data: Option<String>,
}

/// Общие поля Message и InaccessibleMessage (https://core.telegram.org/bots/api#message,
/// https://core.telegram.org/bots/api#inaccessiblemessage). У InaccessibleMessage date всегда 0, текста нет.
#[derive(Debug, Clone, Deserialize)]
pub struct CbMessage {
    pub message_id: i64,
    pub date: i64,
    pub chat: Chat,
    pub text: Option<String>,
}

/// Chat: https://core.telegram.org/bots/api#chat — id (до 52 значащих бит, i64 хватает) и username.
#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    pub username: Option<String>,
}

/// User: https://core.telegram.org/bots/api#user
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    pub first_name: String,
    pub username: Option<String>,
}

impl User {
    /// Кто нажал — для журнала и сообщения: «@имя (id)» или «Имя (id)».
    pub fn describe(&self) -> String {
        match &self.username {
            Some(u) => format!("@{u} ({})", self.id),
            None => format!("{} ({})", self.first_name, self.id),
        }
    }
}

impl Telegram {
    /// sendMessage с inline-клавиатурой (reply_markup = InlineKeyboardMarkup).
    /// Принимает: чат, текст, клавиатуру (notify/callback.rs::keyboard). Ошибки: TgError.
    pub async fn send_with_keyboard(
        &self,
        chat_id: &str,
        text: &str,
        keyboard: Value,
    ) -> Result<(), TgError> {
        let params =
            json!({ "chat_id": chat_id, "text": fit_text(text), "reply_markup": keyboard });
        self.call_with("sendMessage", params, self.timeout())
            .await
            .map(|_| ())
    }

    /// getUpdates — длинный опрос. Таймаут HTTP = таймаут опроса + http.timeout_ms:
    /// Telegram держит запрос до timeout секунд, затем отвечает как обычный запрос.
    /// Принимает: offset (None — с первого неподтверждённого), timeout опроса в секундах.
    /// Возвращает: список Update. Ошибки: TgError (в т.ч. если настроен webhook — замечание 1 в документации).
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_s: u64,
    ) -> Result<Vec<Update>, TgError> {
        let mut params = json!({ "timeout": timeout_s, "allowed_updates": ALLOWED_UPDATES });
        if let Some(o) = offset {
            params["offset"] = json!(o);
        }
        let http_timeout = Duration::from_secs(timeout_s) + self.timeout();
        let r = self.call_with("getUpdates", params, http_timeout).await?;
        serde_json::from_value(r).map_err(|e| TgError::BadResponse(e.to_string()))
    }

    /// answerCallbackQuery — убирает «часики» у нажавшего и показывает короткий ответ.
    /// Принимает: id нажатия, текст (None — без уведомления; предел протокола 0-200 символов,
    /// длиннее обрезается). Ошибки: TgError.
    pub async fn answer_callback(&self, query_id: &str, text: Option<&str>) -> Result<(), TgError> {
        let mut params = json!({ "callback_query_id": query_id });
        if let Some(t) = text {
            // Считаем строже — в единицах UTF-16, как в tg_text.rs.
            let mut units = 0;
            let short: String = t
                .chars()
                .take_while(|c| {
                    units += c.len_utf16();
                    units <= ANSWER_MAX_CHARS
                })
                .collect();
            params["text"] = json!(short);
        }
        self.call_with("answerCallbackQuery", params, self.timeout())
            .await
            .map(|_| ())
    }

    /// editMessageText — новый текст сообщения и пустая inline-клавиатура (кнопки убираются,
    /// второй раз нажать нельзя). Принимает: чат (числовой id из нажатия), id сообщения, текст.
    /// Ошибки: TgError.
    /// Документация не говорит прямо, что пустой inline_keyboard убирает кнопки: InlineKeyboardMarkup —
    /// «Array of button rows», пустой массив — допустимое значение. Проверить на реальном ответе (вопрос в Issue #10).
    pub async fn edit_text_drop_keyboard(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
    ) -> Result<(), TgError> {
        let params = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": fit_text(text),
            "reply_markup": { "inline_keyboard": [] },
        });
        self.call_with("editMessageText", params, self.timeout())
            .await
            .map(|_| ())
    }
}

/// Предел текста answerCallbackQuery — константа протокола: «0-200 characters»,
/// https://core.telegram.org/bots/api#answercallbackquery
const ANSWER_MAX_CHARS: usize = 200;
