//! Файл: venues/bingx/user_key.rs
//! Что делает: ключ потока аккаунта BingX (listenKey, решение 76): получить, продлить, закрыть. Ключ — секрет сессии:
//! в журнал и сырой журнал не пишется (rest.rs не пишет ответы этого пути).
//! Кто вызывает: venues/bingx/user_stream.rs.
//! От чего зависит: rest.rs (подписанный запрос), ctx.rs.
//! Настройки: через rest.rs — venue.rest_url, venue.recv_window_ms; секреты bingx.api_key, bingx.api_secret.
//!
//! Официально (docs-v3, Spot → Listen Key; «Signature Verification: Yes», лимит 2/с):
//! - POST /openApi/user/auth/userDataStream — новый ключ, живёт 1 час
//!   (https://bingx-api.github.io/docs-v3/#/en/Spot/Listen%20Key/Generate%20Listen%20Key). Ответ в таблице —
//!   {code, msg, data:{listenKey}}, в примере — {"listenKey": …}: rest.rs отдаёт data или весь ответ, ключ берётся
//!   из поля listenKey того, что отдано (обе формы документации);
//! - PUT …?listenKey= — продлить до 60 минут от вызова, BingX советует раз в 30 минут
//!   (https://bingx-api.github.io/docs-v3/#/en/Spot/Listen%20Key/Extend%20Listen%20Key%20Validity);
//! - DELETE …?listenKey= — закрыть (https://bingx-api.github.io/docs-v3/#/en/Spot/Listen%20Key/Close%20Listen%20Key).

use reqwest::Method;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::rest;

/// Путь ключа потока аккаунта.
pub const PATH: &str = "/openApi/user/auth/userDataStream";

/// Срок жизни ключа по документации — 1 час: продлевать реже нельзя.
pub const KEY_LIFETIME_S: u64 = 3600;

/// Новый ключ. Ошибки: ключи API, коды BingX; BadResponse — в ответе нет listenKey.
pub async fn create(ctx: &Ctx) -> GatewayResult<Zeroizing<String>> {
    let v = rest::signed(ctx, Method::POST, PATH, Vec::new(), Op::Other).await?;
    v.get("listenKey")
        .and_then(Value::as_str)
        .map(|k| Zeroizing::new(k.to_string()))
        .ok_or_else(|| {
            ctx.tag(crate::gateway::errors::GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "ответ на получение listenKey без поля listenKey",
            ))
        })
}

/// Продлевает ключ. Ошибки: коды BingX (ключа нет — нужен новый).
pub async fn extend(ctx: &Ctx, key: &str) -> GatewayResult<()> {
    let params = vec![("listenKey", key.to_string())];
    rest::signed(ctx, Method::PUT, PATH, params, Op::Other).await?;
    Ok(())
}

/// Закрывает ключ. Ошибок не возвращает: ключ всё равно истечёт через час.
pub async fn close(ctx: &Ctx, key: &str) {
    let params = vec![("listenKey", key.to_string())];
    let _ = rest::signed(ctx, Method::DELETE, PATH, params, Op::Other).await;
}
