//! Файл: venues/xt/futures.rs
//! Что делает: подписанный GET фьючерсов XT.com (панель «Балансы», только чтение) — своя подпись фьючерсов (sign.rs),
//! пауза после 429 отдельно от спота, разбор ответа {returnCode, result} (reply.rs).
//! Кто вызывает: all_balances.rs.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs.
//! Настройки: http.*; секреты xt.api_key, xt.api_secret (адрес передаёт вызывающий).
//!
//! Официально (https://doc.xt.com/#futures_documentationsignSteps, `_futures_documentation/signSteps.md`): заголовки
//! validate-appkey, validate-timestamp, validate-signature; X = «validate-appkey=…&validate-timestamp=…»,
//! Y = «#path#query#body»; адреса https://fapi.xt.com (USDT-M) и https://dapi.xt.com (Coin-M).

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::sign;

/// Подписанный GET фьючерсов XT по адресу base (панель «Балансы», только чтение; своя подпись — sign.rs).
/// Возвращает: result. Ошибки: ключи, связь, ответ фьючерсов.
pub async fn futures_get(ctx: &Ctx, base: &str, path: &str) -> GatewayResult<Value> {
    let (key, secret) = ctx.keys()?;
    let ts = chrono::Utc::now().timestamp_millis();
    let original = format!(
        "{}{}",
        sign::futures_headers(&key, ts),
        sign::data("", path, "", "")
    );
    let signature = sign::hmac_hex(&secret, &original)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::InvalidCredentials, None, m)))?;
    // Решение 201: ручная работа ждёт конец паузы (gateway/patience.rs).
    crate::gateway::patience::wait(super::reply::pause_left_ms(false)).await;
    super::reply::gate(false, path)?;
    let lim = ctx.limits()?;
    let url = format!("{}{path}", base.trim_end_matches('/'));
    let req = ctx
        .client()?
        .get(url)
        .header("validate-appkey", key.as_str())
        .header("validate-timestamp", ts.to_string())
        .header("validate-signature", signature);
    match crate::gateway::http::exchange_full(req, lim).await {
        Ok(r) => super::reply::parse_futures(ctx, path, &r),
        Err(f) => Err(super::errors::from_fail(&f, &lim)),
    }
}
