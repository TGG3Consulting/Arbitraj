//! Файл: notify/check.rs
//! Что делает: кнопка «Проверить подключение» для Telegram: проверяет токен (getMe)
//! и доступ бота к чату (getChat). Ответ — «Подключено» или точная причина сбоя.
//! Кто вызывает: api/check.rs.
//! От чего зависит: notify/client.rs, notify/telegram.rs.
//! Настройки: notify.telegram_api_url, http.timeout_ms, notify.telegram_chat; секрет telegram.bot_token.

use crate::gateway::contract::ConnectionCheck;
use crate::secrets::Vault;
use crate::settings::store::Store;

use super::client;

/// Проверяет подключение к Telegram.
/// Принимает: HTTP-клиент, настройки, секреты.
/// Возвращает: ConnectionCheck — ok=true «Подключено: бот @имя, чат …», иначе точная причина.
/// Не падает: любая ошибка превращается в понятный текст причины.
pub async fn telegram(http: &reqwest::Client, settings: &Store, vault: &Vault) -> ConnectionCheck {
    let ready = match client::build(http, settings, vault) {
        Ok(r) => r,
        Err(e) => {
            return ConnectionCheck {
                ok: false,
                details: e.message,
            };
        }
    };
    let bot = match ready.tg.get_me().await {
        Ok(name) => name,
        Err(e) => {
            return ConnectionCheck {
                ok: false,
                details: format!("Токен бота: {e}"),
            };
        }
    };
    match ready.tg.get_chat(&ready.chat).await {
        Ok(chat) => ConnectionCheck {
            ok: true,
            details: format!("Подключено: бот @{bot}, чат «{chat}»"),
        },
        Err(e) => ConnectionCheck {
            ok: false,
            details: format!("Бот @{bot} работает, но чат {} недоступен: {e}", ready.chat),
        },
    }
}
