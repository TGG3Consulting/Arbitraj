//! Файл: notify/telegram.rs
//! Что делает: обращения к Telegram Bot API — проверка бота (getMe), проверка чата (getChat),
//! отправка сообщения (sendMessage). Ответ Telegram разбирается по официальной документации.
//! Кто вызывает: notify/dispatch.rs (отправка), notify/check.rs (проверка подключения).
//! От чего зависит: reqwest.
//! Настройки: адрес API и таймаут передаёт вызывающий (notify.telegram_api_url, http.timeout_ms).
//!
//! Официальная документация: https://core.telegram.org/bots/api
//! - Формат запроса: https://core.telegram.org/bots/api#making-requests
//!   «https://api.telegram.org/bot<token>/METHOD_NAME»; ответ — JSON с полем «ok»,
//!   при ok=false — «description» (понятная причина) и «error_code».
//! - getMe: https://core.telegram.org/bots/api#getme
//! - getChat: https://core.telegram.org/bots/api#getchat
//! - sendMessage: https://core.telegram.org/bots/api#sendmessage
//!
//! Токен никогда не попадает в журнал и ошибки: из ошибок reqwest адрес удаляется (without_url).

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

/// Предел длины текста sendMessage — константа протокола:
/// «text … 1-4096 characters after entities parsing», https://core.telegram.org/bots/api#sendmessage
const MAX_TEXT_CHARS: usize = 4096;

/// Ответ Bot API (раздел «Making requests»).
#[derive(Debug, Deserialize)]
struct ApiResponse {
    ok: bool,
    result: Option<Value>,
    description: Option<String>,
    error_code: Option<i64>,
    /// Поле «parameters» (ResponseParameters): retry_after — через сколько секунд можно повторить,
    /// migrate_to_chat_id — новый ID чата после перевода группы в супергруппу.
    parameters: Option<ResponseParameters>,
}

/// ResponseParameters: https://core.telegram.org/bots/api#responseparameters
#[derive(Debug, Deserialize)]
struct ResponseParameters {
    retry_after: Option<i64>,
    migrate_to_chat_id: Option<i64>,
}

/// Ошибка обращения к Telegram с понятной причиной.
#[derive(Debug, thiserror::Error)]
pub enum TgError {
    /// Telegram ответил ok=false — причина из поля description.
    #[error("Telegram отказал: {description}{details}")]
    Api {
        description: String,
        details: String,
    },
    /// Нет ответа за отведённое время.
    #[error("Telegram не ответил за отведённое время")]
    Timeout,
    /// Нет связи (DNS, TCP, TLS) — адрес и токен в текст не попадают.
    #[error("Нет связи с Telegram: {0}")]
    Network(String),
    /// Ответ не в формате Bot API.
    #[error("Непонятный ответ Telegram: {0}")]
    BadResponse(String),
}

/// Клиент Bot API.
pub struct Telegram {
    http: reqwest::Client,
    base_url: String,
    token: String,
    timeout: Duration,
}

impl Telegram {
    /// Создаёт клиента.
    /// Принимает: общий HTTP-клиент, адрес API (из настроек), токен (из секретов), таймаут (из настроек).
    pub fn new(http: reqwest::Client, base_url: &str, token: String, timeout: Duration) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            timeout,
        }
    }

    /// Вызывает метод Bot API.
    /// Принимает: имя метода и параметры JSON. Возвращает: поле result. Ошибки: TgError.
    async fn call(&self, method: &str, params: Value) -> Result<Value, TgError> {
        let url = format!("{}/bot{}/{}", self.base_url, self.token, method);
        let resp = self
            .http
            .post(url)
            .timeout(self.timeout)
            .json(&params)
            .send()
            .await
            .map_err(net_error)?;
        // Bot API отдаёт JSON и при ошибках (4xx), поэтому статус не проверяем отдельно.
        let body: ApiResponse = resp
            .json()
            .await
            .map_err(|e| TgError::BadResponse(e.without_url().to_string()))?;
        if body.ok {
            body.result
                .ok_or_else(|| TgError::BadResponse("нет поля result".into()))
        } else {
            Err(TgError::Api {
                description: body.description.unwrap_or_else(|| "без описания".into()),
                details: details(body.error_code, body.parameters.as_ref()),
            })
        }
    }

    /// getMe — проверка токена. Возвращает: имя пользователя бота. Ошибки: TgError.
    pub async fn get_me(&self) -> Result<String, TgError> {
        let r = self.call("getMe", json!({})).await?;
        Ok(r.get("username")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// getChat — проверка, что бот видит чат. Возвращает: название чата или имя. Ошибки: TgError.
    pub async fn get_chat(&self, chat_id: &str) -> Result<String, TgError> {
        let r = self.call("getChat", json!({ "chat_id": chat_id })).await?;
        let name = ["title", "username", "first_name"]
            .iter()
            .find_map(|k| r.get(*k).and_then(Value::as_str))
            .unwrap_or(chat_id);
        Ok(name.to_string())
    }

    /// sendMessage — отправка текста. Текст длиннее предела протокола обрезается с пометкой «…».
    /// Ошибки: TgError.
    pub async fn send(&self, chat_id: &str, text: &str) -> Result<(), TgError> {
        // Считаем строже — в единицах UTF-16 (эмодзи = 2): так предел не будет превышен
        // при любом способе подсчёта на стороне Telegram.
        let text = if text.encode_utf16().count() > MAX_TEXT_CHARS {
            let mut t = String::new();
            let mut units = 0;
            for ch in text.chars() {
                units += ch.len_utf16();
                if units > MAX_TEXT_CHARS - 1 {
                    break;
                }
                t.push(ch);
            }
            t.push('…');
            t
        } else {
            text.to_string()
        };
        self.call("sendMessage", json!({ "chat_id": chat_id, "text": text }))
            .await
            .map(|_| ())
    }
}

/// Переводит сетевую ошибку reqwest в TgError без адреса (в адресе токен).
fn net_error(e: reqwest::Error) -> TgError {
    if e.is_timeout() {
        TgError::Timeout
    } else {
        TgError::Network(e.without_url().to_string())
    }
}

/// Дополнения к причине отказа: код ошибки и параметры ответа, если Telegram их прислал.
fn details(code: Option<i64>, p: Option<&ResponseParameters>) -> String {
    let mut out = String::new();
    if let Some(c) = code {
        out.push_str(&format!(" (код {c})"));
    }
    if let Some(s) = p.and_then(|p| p.retry_after) {
        out.push_str(&format!(", повторить можно через {s} с"));
    }
    if let Some(id) = p.and_then(|p| p.migrate_to_chat_id) {
        out.push_str(&format!(", группа стала супергруппой — новый ID чата {id}"));
    }
    out
}
