//! Файл: venues/mexc/user_key.rs
//! Что делает: ключ потока аккаунта MEXC (listenKey): получить, продлить, закрыть. Ключ — секрет сессии: в журнал и
//! сырой журнал не пишется (rest.rs не пишет ответы этого пути).
//! Кто вызывает: venues/mexc/user_stream.rs.
//! От чего зависит: rest.rs (подписанный запрос), ctx.rs.
//! Настройки: через rest.rs — venue.rest_url, venue.recv_window_ms; секреты mexc.api_key, mexc.api_secret.
//!
//! Официально (https://www.mexc.com/api-docs/spot-v3/websocket-user-data-streams/listen-key):
//! - POST /api/v3/userDataStream (SPOT_ACCOUNT_R) — новый ключ {"listenKey"}, живёт 60 минут;
//! - PUT …?listenKey= — продлить на 60 минут от вызова, MEXC советует раз в 30 минут;
//! - DELETE …?listenKey= — закрыть. «Each UID can apply for a maximum of 60 listen keys (excluding invalid listen
//!   keys)», «Each listen key maximum support 5 websocket connection».

use reqwest::Method;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::errors::Op;
use super::rest;

/// Путь ключа потока аккаунта.
pub const PATH: &str = "/api/v3/userDataStream";

/// Срок жизни ключа по документации — 60 минут: продлевать реже нельзя.
pub const KEY_LIFETIME_S: u64 = 3600;

/// Новый ключ. Ошибки: ключи API, коды MEXC; BadResponse — в ответе нет listenKey.
pub async fn create(ctx: &Ctx) -> GatewayResult<Zeroizing<String>> {
    let v = rest::signed(ctx, Method::POST, PATH, Vec::new(), Op::Other).await?;
    v.get("listenKey")
        .and_then(Value::as_str)
        .map(|k| Zeroizing::new(k.to_string()))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "ответ на получение listenKey без поля listenKey",
            ))
        })
}

/// Продлевает ключ. Ошибки: коды MEXC (ключа нет — нужен новый).
pub async fn extend(ctx: &Ctx, key: &str) -> GatewayResult<()> {
    let params = vec![("listenKey", key.to_string())];
    rest::signed(ctx, Method::PUT, PATH, params, Op::Other).await?;
    Ok(())
}

/// Закрывает ключ. Ошибок не возвращает: сбой закрытия — запись в журнал (решение 118, А-🟠10: незакрытые ключи
/// копятся до предела 60 на UID), ключ всё равно истечёт через час. Сам ключ в текст не попадает.
pub async fn close(ctx: &Ctx, key: &str) {
    let params = vec![("listenKey", key.to_string())];
    if let Err(e) = rest::signed(ctx, Method::DELETE, PATH, params, Op::Other).await {
        let msg = format!(
            "Поток аккаунта: listenKey не закрыт: {} — истечёт сам через час (не больше 60 ключей на UID)",
            e.message.replace(key, "***")
        );
        let _ = ctx
            .journal
            .write(Level::Warn, "venue.user_stream", Some(ID), msg, None)
            .await;
    }
}
