//! Файл: notify/ctl/kb.rs
//! Что делает: общий рендер клавиатур чата «Управление» (решение 176): кнопка, ряды, нижний ряд навигации «Назад» и
//! «Главное меню» (есть на каждом экране), экран как пара «текст + клавиатура». callback_data собираются здесь и
//! проверяются на предел протокола (1–64 байта, notify/callback.rs::CALLBACK_DATA_MAX_BYTES).
//! Кто вызывает: notify/ctl/screens.rs, balances.rs, transfer.rs.
//! От чего зависит: notify/callback.rs (предел), serde_json.
//! Настройки: нет.
//!
//! InlineKeyboardMarkup / InlineKeyboardButton (text, callback_data, url):
//! https://core.telegram.org/bots/api#inlinekeyboardmarkup

use serde_json::{Value, json};

use super::super::callback::CALLBACK_DATA_MAX_BYTES;

/// Экран: текст и ряды кнопок.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    pub text: String,
    pub rows: Vec<Vec<Value>>,
}

/// Кнопка с callback_data. Длиннее предела протокола — кнопки нет (None), причина — у вызывающего в тексте.
/// Принимает: надпись, callback_data. Ошибок нет.
pub fn btn(text: &str, data: &str) -> Option<Value> {
    (!data.is_empty() && data.len() <= CALLBACK_DATA_MAX_BYTES)
        .then(|| json!({ "text": text, "callback_data": data }))
}

/// Кнопка-ссылка (поле url). Принимает: надпись, адрес. Ошибок нет.
pub fn link(text: &str, url: &str) -> Value {
    json!({ "text": text, "url": url })
}

impl Screen {
    /// Новый экран с текстом. Ошибок нет.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            rows: Vec::new(),
        }
    }

    /// Ряд из одной кнопки (None — пропускается). Ошибок нет.
    pub fn one(mut self, b: Option<Value>) -> Self {
        if let Some(b) = b {
            self.rows.push(vec![b]);
        }
        self
    }

    /// Ряд из нескольких кнопок (пустые пропускаются; ряд без кнопок не добавляется). Ошибок нет.
    pub fn row(mut self, bs: Vec<Option<Value>>) -> Self {
        let r: Vec<Value> = bs.into_iter().flatten().collect();
        if !r.is_empty() {
            self.rows.push(r);
        }
        self
    }

    /// Добавляет строку текста. Ошибок нет.
    pub fn line(mut self, t: impl AsRef<str>) -> Self {
        self.text.push('\n');
        self.text.push_str(t.as_ref());
        self
    }

    /// Нижний ряд навигации: «« Назад» (callback back; None — без него) и «Главное меню». Ошибок нет.
    pub fn nav(self, back: Option<&str>) -> Self {
        let back = back.and_then(|d| btn("« Назад", d));
        self.row(vec![back, btn("Главное меню", "k:root")])
    }

    /// InlineKeyboardMarkup экрана. Ошибок нет.
    pub fn markup(&self) -> Value {
        json!({ "inline_keyboard": self.rows })
    }
}
