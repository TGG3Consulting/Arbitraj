//! Файл: notify/callback.rs
//! Что делает: кнопки под сообщением Telegram — собирает inline-клавиатуру события из каталога,
//! упаковывает в callback_data «код кнопки + id объекта» и разбирает обратно; превращает нажатую
//! кнопку в команду ядру (ManualAction).
//! Кто вызывает: notify/dispatch.rs (клавиатура), notify/press.rs (разбор нажатия).
//! От чего зависит: notify/catalog.rs (кнопки событий), core/types/control.rs (ManualAction).
//! Настройки: нет.
//!
//! Формат callback_data: «<код кнопки>:<id объекта>», например «sell_now:<id алерта>».
//! Коды кнопок из каталога двоеточий не содержат, поэтому делим по первому «:» — id может быть любым.
//! Предел протокола: callback_data «1-64 bytes», https://core.telegram.org/bots/api#inlinekeyboardbutton

use serde_json::{Value, json};

use crate::core::types::ManualAction;

use super::catalog::EventDef;

/// Предел длины callback_data в байтах — константа протокола (InlineKeyboardButton, «1-64 bytes»).
pub const CALLBACK_DATA_MAX_BYTES: usize = 64;

/// Разделитель кода кнопки и id объекта в callback_data — формат самой системы (см. шапку).
const SEP: char = ':';

/// Ошибка сборки кнопок.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CallbackError {
    /// callback_data длиннее предела протокола.
    #[error("кнопка «{button}»: callback_data {len} байт, а Telegram принимает не больше {max}")]
    TooLong {
        button: String,
        len: usize,
        max: usize,
    },
    /// Пустой id объекта — нажатие не к чему привязать.
    #[error("кнопка «{0}»: пустой id объекта")]
    EmptyObject(String),
}

/// Упаковывает код кнопки и id объекта в callback_data.
/// Принимает: код кнопки (из каталога), id объекта (например id алерта).
/// Возвращает: строку callback_data. Ошибки: EmptyObject, TooLong (предел 64 байта).
pub fn encode(button: &str, object_id: &str) -> Result<String, CallbackError> {
    if object_id.is_empty() {
        return Err(CallbackError::EmptyObject(button.to_string()));
    }
    let data = format!("{button}{SEP}{object_id}");
    if data.len() > CALLBACK_DATA_MAX_BYTES {
        return Err(CallbackError::TooLong {
            button: button.to_string(),
            len: data.len(),
            max: CALLBACK_DATA_MAX_BYTES,
        });
    }
    Ok(data)
}

/// Разбирает callback_data. Принимает: строку из нажатия.
/// Возвращает: (код кнопки, id объекта) или None — строка не в формате системы.
pub fn decode(data: &str) -> Option<(&str, &str)> {
    let (button, object) = data.split_once(SEP)?;
    if button.is_empty() || object.is_empty() {
        return None;
    }
    Some((button, object))
}

/// Собирает InlineKeyboardMarkup для события: все кнопки события в одном ряду.
/// https://core.telegram.org/bots/api#inlinekeyboardmarkup — inline_keyboard: массив рядов кнопок;
/// кнопка — text и callback_data (https://core.telegram.org/bots/api#inlinekeyboardbutton).
/// Принимает: событие из каталога, id объекта. Возвращает: Some(клавиатура) или None — у события нет кнопок.
/// Ошибки: как у encode.
pub fn keyboard(def: &EventDef, object_id: &str) -> Result<Option<Value>, CallbackError> {
    if def.buttons.is_empty() {
        return Ok(None);
    }
    let row = def
        .buttons
        .iter()
        .map(|b| Ok(json!({ "text": b.title, "callback_data": encode(b.code, object_id)? })))
        .collect::<Result<Vec<Value>, CallbackError>>()?;
    Ok(Some(json!({ "inline_keyboard": [row] })))
}

/// Превращает нажатую кнопку в команду ядру (решение 37).
/// Принимает: код кнопки и id объекта из callback_data.
/// Возвращает: ManualAction или None — у системы нет такой кнопки.
/// Коды — из notify/catalog.rs (STUCK_BUTTONS): «sell_now» и «take_over» у stuck.loss_alert.
pub fn to_action(button: &str, object_id: &str) -> Option<ManualAction> {
    let alert_id = object_id.to_string();
    match button {
        "sell_now" => Some(ManualAction::SellNow { alert_id }),
        "take_over" => Some(ManualAction::TakeOver { alert_id }),
        _ => None,
    }
}

/// Название кнопки по коду — для ответа нажавшему и правки сообщения.
/// Принимает: код кнопки. Возвращает: надпись из каталога или None.
pub fn title(button: &str) -> Option<&'static str> {
    super::catalog::EVENTS
        .iter()
        .flat_map(|e| e.buttons.iter())
        .find(|b| b.code == button)
        .map(|b| b.title)
}
