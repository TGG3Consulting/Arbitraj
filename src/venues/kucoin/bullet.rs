//! Файл: venues/kucoin/bullet.rs
//! Что делает: токен подключения WebSocket KuCoin — публичный (bullet-public, рынок) и приватный (bullet-private, поток
//! аккаунта): адрес сервера, токен, интервал пинга и срок ожидания понга — из ответа (не настройки), адрес подключения
//! «endpoint?token=…&connectId=…». Токен — секрет сессии: в журнал и сырой журнал не пишется (reply.rs, путь /bullet-).
//! Кто вызывает: ws.rs, feed.rs (рынок), user_stream.rs (аккаунт).
//! От чего зависит: rest.rs, num.rs, ctx.rs, getrandom.
//! Настройки: нет своих (адрес REST — venue.rest_url).
//!
//! Официально:
//! - Get Public Token POST /api/v1/bullet-public (Public, вес 10) https://www.kucoin.com/docs-new/websocket-api/base-info/get-public-token-spot-margin
//!   и Get Private Token POST /api/v1/bullet-private (подписанный) — data.token, data.instanceServers[]: endpoint («the URL
//!   may change» — брать из ответа), encrypt, protocol, pingInterval (мс), pingTimeout (мс). Запись ответа:
//!   tests/data/cex_samples_2026_11_04/kucoin_bullet_public.json (endpoint wss://ws-api-spot.kucoin.com/, 18000 / 10000).
//! - Introduction https://www.kucoin.com/docs-new/websocket-api/base-info/introduction — адрес
//!   «wss://…?token=xxx&connectId=xxxxx», connectId — свой уникальный ID; токен действует 24 часа.

use std::time::Duration;

use reqwest::Method;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::num;
use super::reply::Pool;
use super::rest::{self, Req};

/// Токен и параметры сервера.
pub struct Token {
    /// Адрес подключения с токеном и connectId (секрет: не писать в журнал).
    pub url: Zeroizing<String>,
    /// Слать ping не реже (pingInterval).
    pub ping: Duration,
    /// Сколько ждать pong (pingTimeout).
    pub ping_timeout: Duration,
}

/// Что делает: получает токен. Принимает: приватный ли. Возвращает: Token (первый сервер из instanceServers).
/// Ошибки: связь, коды KuCoin, ключи (приватный); BadResponse — нет токена или сервера.
pub async fn get(ctx: &Ctx, private: bool) -> GatewayResult<Token> {
    let r = Req {
        method: Method::POST,
        path: if private {
            "/api/v1/bullet-private"
        } else {
            "/api/v1/bullet-public"
        },
        query: vec![],
        body: None,
        op: Op::Other,
        pool: if private { Pool::Spot } else { Pool::Public },
        signed: private,
        heavy: false,
    };
    let data = rest::send(ctx, r).await?;
    parse(&data, &connect_id()?).map_err(|e| ctx.tag(e))
}

/// Разбор ответа bullet. Принимает: data, connectId. Возвращает: Token. Ошибка BadResponse (токена в тексте нет).
pub fn parse(data: &Value, connect_id: &str) -> GatewayResult<Token> {
    let bad =
        |w: &str| GatewayError::new(ErrorCode::BadResponse, None, format!("KuCoin bullet: {w}"));
    let token = data
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("нет token"))?;
    let srv = data
        .get("instanceServers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| bad("нет instanceServers"))?;
    let endpoint = num::text_opt(srv, "endpoint").ok_or_else(|| bad("нет endpoint"))?;
    let ms = |f: &str| {
        num::text_opt(srv, f)
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .map(Duration::from_millis)
            .ok_or_else(|| bad(&format!("нет {f}")))
    };
    Ok(Token {
        url: Zeroizing::new(format!("{endpoint}?token={token}&connectId={connect_id}")),
        ping: ms("pingInterval")?,
        ping_timeout: ms("pingTimeout")?,
    })
}

/// Свой уникальный connectId — 16 случайных байт в hex. Ошибка Internal — нет источника случайных чисел.
fn connect_id() -> GatewayResult<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| {
        GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("генератор случайных чисел: {e}"),
        )
    })?;
    Ok(hex::encode(b))
}
