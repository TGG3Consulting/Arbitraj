//! Получатели и клиент дополнительных исходящих Telegram-рассылок.
//! Канал не является ролью управления и не запускает getUpdates.
use std::time::Duration;

use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayError;
use crate::secrets::{TELEGRAM_DAILY_TOKEN, TELEGRAM_DUTY_TOKEN, Vault};
use crate::settings::{read::At, store::Store};

use super::telegram::Telegram;

#[derive(Clone, Copy)]
pub enum Channel {
    Duty,
    Daily,
}

impl Channel {
    fn keys(self) -> (&'static str, &'static str) {
        match self {
            Self::Duty => ("telegram.duty_chat_ids", TELEGRAM_DUTY_TOKEN),
            Self::Daily => ("telegram.daily_chat_ids", TELEGRAM_DAILY_TOKEN),
        }
    }
}

/// Берёт текущую конфигурацию из памяти. HTTP вызывается только при проверке или отправке.
pub fn build(
    http: &reqwest::Client,
    settings: &Store,
    vault: &Vault,
    channel: Channel,
) -> Result<(Telegram, Vec<String>), GatewayError> {
    let (chats_key, token_key) = channel.keys();
    let at = At::default();
    let chats = settings.list(chats_key, at)?;
    if chats.is_empty() {
        return Err(GatewayError::not_set(None, chats_key, "Не заданы ID чатов"));
    }
    let url = settings.text("notify.telegram_api_url", at)?;
    let timeout = Duration::from_millis(settings.integer("http.timeout_ms", at)?);
    Ok((
        Telegram::new(http.clone(), &url, vault.get(token_key)?, timeout),
        chats,
    ))
}

/// Проверка по кнопке владельца: токен и тестовое сообщение каждому адресату.
pub async fn check(
    http: &reqwest::Client,
    settings: &Store,
    vault: &Vault,
    channel: Channel,
) -> ConnectionCheck {
    let failure = |details: String| ConnectionCheck { ok: false, details };
    let (tg, chats) = match build(http, settings, vault, channel) {
        Ok(config) => config,
        Err(e) => return failure(e.message),
    };
    let bot = match tg.get_me().await {
        Ok(name) => name,
        Err(e) => return failure(format!("Проверка токена: {e}")),
    };
    let mut failed = Vec::new();
    for chat in &chats {
        let sent = match channel {
            Channel::Duty => tg.send(chat, "ARB • Проверка доставки дежурному").await,
            Channel::Daily => tg
                .send_document(
                    chat,
                    "arb-check.txt",
                    "text/plain",
                    b"ARB Telegram document delivery check".to_vec(),
                    "ARB • Проверка доставки файлов автоотчётов",
                )
                .await
                .map(|_| ()),
        };
        if let Err(e) = sent {
            failed.push(format!("{chat}: {e}"));
        }
    }
    if failed.is_empty() {
        ConnectionCheck {
            ok: true,
            details: format!("Подключено: @{bot}, чатов проверено: {}", chats.len()),
        }
    } else {
        failure(format!(
            "Бот @{bot}, Telegram принял проверку доставки: {} из {}. {}",
            chats.len() - failed.len(),
            chats.len(),
            failed.join("; ")
        ))
    }
}
