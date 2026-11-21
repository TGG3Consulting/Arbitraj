//! Файл: notify/tg_html.rs
//! Что делает: решение 251 — отправка и правка сообщений с разметкой HTML (parse_mode «HTML»): текст уже собран
//! шаблоном notify/alert.rs (экранирован и подогнан под предел длины до разметки — здесь его не режем, иначе можно
//! разрезать тег).
//! Кто вызывает: notify/dispatch.rs, dispatch_role.rs, mirror.rs, mirror_send.rs.
//! От чего зависит: notify/telegram.rs (call_with), notify/tg_methods.rs (no_keyboard).
//! Настройки: таймаут — http.timeout_ms (у клиента).
//!
//! Официальная документация (прочитана 10.11.2026):
//! - sendMessage, parse_mode «Mode for parsing entities in the message text»: https://core.telegram.org/bots/api#sendmessage
//! - editMessageText (chat_id, message_id, text, parse_mode, reply_markup; «On success, if the edited message is not
//!   an inline message, the edited Message is returned»): https://core.telegram.org/bots/api#editmessagetext
//! - HTML style: https://core.telegram.org/bots/api#formatting-options

use serde_json::{Value, json};

use super::telegram::{Telegram, TgError};
use super::tg_methods::no_keyboard;

/// Значение parse_mode для HTML-разметки (Formatting options → HTML style).
const PARSE_MODE: &str = "HTML";

impl Telegram {
    /// sendMessage с parse_mode HTML и (если есть) кнопками. Принимает: чат, HTML (notify/alert.rs), reply_markup.
    /// Возвращает: message_id. Ошибки: TgError (в т.ч. отказ разбора разметки — причина в description).
    pub async fn send_html(
        &self,
        chat: &str,
        html: &str,
        markup: Option<Value>,
    ) -> Result<i64, TgError> {
        let mut p = json!({ "chat_id": chat, "text": html, "parse_mode": PARSE_MODE });
        if let Some(m) = markup {
            p["reply_markup"] = m;
        }
        let r = self.call_with("sendMessage", p, self.timeout()).await?;
        r.get("message_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| TgError::BadResponse("в ответе sendMessage нет message_id".into()))
    }

    /// editMessageText с parse_mode HTML и новой клавиатурой (None — кнопки убираются).
    /// Принимает: чат, id сообщения, HTML, клавиатуру. Ошибки: TgError (в т.ч. «message is not modified»).
    pub async fn edit_html(
        &self,
        chat: &str,
        message_id: i64,
        html: &str,
        markup: Option<Value>,
    ) -> Result<(), TgError> {
        let p = json!({
            "chat_id": chat, "message_id": message_id, "text": html, "parse_mode": PARSE_MODE,
            "reply_markup": markup.unwrap_or_else(no_keyboard),
        });
        self.call_with("editMessageText", p, self.timeout())
            .await
            .map(|_| ())
    }
}
