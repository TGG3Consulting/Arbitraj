//! Файл: notify/tg_methods.rs
//! Что делает: обращения к Telegram Bot API для записей «Ручного управления» и меню (решения 156, 157):
//! отправка с любой разметкой (inline-кнопки или ForceReply) и номером сообщения в ответе, правка текста
//! с кнопками, правка только кнопок, регистрация списка команд (подсказка по «/»).
//! Кто вызывает: notify/mirror.rs, notify/press*.rs, notify/menu_press.rs, notify/tg_commands.rs, notify/updates.rs.
//! От чего зависит: notify/telegram.rs (call_with), notify/tg_text.rs.
//! Настройки: таймаут — http.timeout_ms (у клиента).
//!
//! Официальная документация: https://core.telegram.org/bots/api
//! - sendMessage (reply_markup, reply_parameters; в ответе — Message): https://core.telegram.org/bots/api#sendmessage
//! - ReplyParameters (message_id): https://core.telegram.org/bots/api#replyparameters
//! - ForceReply (force_reply, input_field_placeholder 1-64 символа): https://core.telegram.org/bots/api#forcereply
//! - editMessageText: https://core.telegram.org/bots/api#editmessagetext
//! - editMessageReplyMarkup: https://core.telegram.org/bots/api#editmessagereplymarkup
//! - setMyCommands / BotCommand (command 1-32 символа: строчные латинские, цифры, «_»; description 1-256):
//!   https://core.telegram.org/bots/api#setmycommands, https://core.telegram.org/bots/api#botcommand
//!
//! Страницы документации в этой сессии разработки открыть не удалось (сеть) — поля взяты по Bot API, проверить на
//! первом реальном ответе Telegram.

use serde_json::{Value, json};

use super::telegram::{Telegram, TgError};
use super::tg_text::fit_text;

/// Пустая inline-клавиатура — убирает кнопки у сообщения (как в tg_buttons.rs::edit_text_drop_keyboard).
pub fn no_keyboard() -> Value {
    json!({ "inline_keyboard": [] })
}

/// ForceReply: Telegram откроет у человека ответ на это сообщение. Принимает: подсказку в поле ввода (≤ 64 символа).
pub fn force_reply(placeholder: &str) -> Value {
    let p: String = placeholder.chars().take(64).collect();
    json!({ "force_reply": true, "input_field_placeholder": p })
}

impl Telegram {
    /// sendMessage с разметкой и (если задано) ответом на сообщение.
    /// Принимает: чат (значение notify.telegram_chat или id), текст, reply_markup, id сообщения для ответа.
    /// Возвращает: message_id отправленного сообщения. Ошибки: TgError.
    pub async fn send_markup(
        &self,
        chat: &str,
        text: &str,
        markup: Option<Value>,
        reply_to: Option<i64>,
    ) -> Result<i64, TgError> {
        let mut p = json!({ "chat_id": chat, "text": fit_text(text) });
        if let Some(m) = markup {
            p["reply_markup"] = m;
        }
        if let Some(id) = reply_to {
            // allow_sending_without_reply — сообщение уйдёт, даже если исходное удалено.
            p["reply_parameters"] =
                json!({ "message_id": id, "allow_sending_without_reply": true });
        }
        let r = self.call_with("sendMessage", p, self.timeout()).await?;
        r.get("message_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| TgError::BadResponse("в ответе sendMessage нет message_id".into()))
    }

    /// editMessageText с новой клавиатурой (None — кнопки убираются).
    /// Принимает: чат, id сообщения, текст, клавиатуру. Ошибки: TgError (в т.ч. «message is not modified»).
    pub async fn edit_text_markup(
        &self,
        chat: &str,
        message_id: i64,
        text: &str,
        markup: Option<Value>,
    ) -> Result<(), TgError> {
        let p = json!({
            "chat_id": chat, "message_id": message_id, "text": fit_text(text),
            "reply_markup": markup.unwrap_or_else(no_keyboard),
        });
        self.call_with("editMessageText", p, self.timeout())
            .await
            .map(|_| ())
    }

    /// editMessageReplyMarkup — только кнопки (None — убрать). Принимает: чат, id сообщения, клавиатуру.
    /// Ошибки: TgError.
    pub async fn edit_markup(
        &self,
        chat: &str,
        message_id: i64,
        markup: Option<Value>,
    ) -> Result<(), TgError> {
        let p = json!({
            "chat_id": chat, "message_id": message_id,
            "reply_markup": markup.unwrap_or_else(no_keyboard),
        });
        self.call_with("editMessageReplyMarkup", p, self.timeout())
            .await
            .map(|_| ())
    }

    /// setMyCommands — список команд бота (подсказка по «/»). Принимает: пары (команда без «/», описание).
    /// Ошибки: TgError.
    pub async fn set_my_commands(&self, commands: &[(&str, &str)]) -> Result<(), TgError> {
        let list: Vec<Value> = commands
            .iter()
            .map(|(c, d)| json!({ "command": c, "description": d }))
            .collect();
        self.call_with("setMyCommands", json!({ "commands": list }), self.timeout())
            .await
            .map(|_| ())
    }
}

/// Пауза, которую велел Telegram (поле parameters.retry_after, ResponseParameters,
/// https://core.telegram.org/bots/api#responseparameters). Принимает: ошибку. Возвращает: секунды или None.
pub fn retry_after(e: &TgError) -> Option<u64> {
    match e {
        TgError::Api {
            retry_after: Some(s),
            ..
        } if *s > 0 => Some(s.unsigned_abs()),
        _ => None,
    }
}
