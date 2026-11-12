//! Файл: venues/mexc/reply.rs
//! Что делает: разбор ответа REST MEXC (код HTTP, тело, Retry-After) в значение или единую ошибку шлюза и пауза
//! запросов по Retry-After после 418/429 (решение 118, А-🔴3, А-🟡16). Ответы пишутся в сырой журнал (raw.*), кроме
//! пути listenKey.
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, user_key.rs (путь), gateway/http.rs (Reply).
//! Настройки: нет.
//!
//! Официальная документация, «HTTP Return Codes» https://www.mexc.com/api-docs/spot-v3/introduction: «HTTP 429
//! indicates the request rate limit is being exceeded», «HTTP 403 … WAF», «Responses with 418 or 429 include a
//! `Retry-After` header indicating the wait time in seconds»; продолжать запросы после 429 — бан IP «from a minimum of
//! 2 minutes to a maximum of 3 days». Поэтому 418/429 — всегда RateLimited по статусу, независимо от тела, а до
//! истечения Retry-After адаптер сам не шлёт ни одного запроса (пауза не короче Retry-After при любом правиле шлюза).
//! Общей схемы ответа нет: успех — объект или массив без поля code либо обёртка {code: 0|200, data, msg}; ошибка —
//! {code, msg}. Ответ с code не 0/200 считается ошибкой и без msg (схемы ответа-ошибки в документации нет).

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// До какого момента (unix мс) запросы к MEXC не отправляются — по последнему Retry-After. Один адаптер MEXC на
/// процесс, поэтому состояние модуля. 0 — паузы нет.
static PAUSE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// Что делает: остаток паузы после 429, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201,
/// gateway/patience.rs). Ошибок нет.
pub fn pause_left_ms() -> i64 {
    PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis()
}

/// Что делает: не пускает запрос, пока идёт пауза по Retry-After.
/// Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate() -> GatewayResult<()> {
    let left = PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!(
                "MEXC: пауза по Retry-After после HTTP 418/429 — ещё {} мс запросы не отправляются",
                left
            ),
        ));
    }
    Ok(())
}

/// Что делает: ставит паузу на Retry-After секунд от текущего момента (дальняя из старой и новой).
/// Принимает: секунды. Ошибок нет.
fn pause(secs: u64) {
    let ms = i64::try_from(secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    let until = chrono::Utc::now().timestamp_millis().saturating_add(ms);
    PAUSE_UNTIL_MS.fetch_max(until, Ordering::Relaxed);
}

/// Что делает: разбор ответа.
/// Принимает: окружение, путь (для журнала), ответ транспорта, операцию.
/// Возвращает: значение (поле data, если ответ в обёртке {code, data}).
/// Ошибки: RateLimited (HTTP 418/429, code 429), InsufficientPermissions (HTTP 403 — WAF), коды MEXC (errors.rs),
/// BadResponse — тело не JSON или непонятный ответ.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    if !path.contains(super::user_key::PATH) {
        ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    }
    let status = r.status.as_u16();
    // Лимит и WAF — по статусу всегда, что бы ни было в теле.
    if matches!(status, 418 | 429 | 403) {
        if status != 403
            && let Some(s) = r.retry_after_s
        {
            pause(s);
        }
        return Err(errors::from_http(status, &r.text, r.retry_after_s));
    }
    let Ok(v) = serde_json::from_str::<Value>(&r.text) else {
        return Err(errors::from_http(status, &r.text, r.retry_after_s));
    };
    let code = v.get("code").and_then(Value::as_i64);
    match code {
        // Решение 131: поле data забирается из ответа, а не копируется (у config/getall это ~5.6 МБ).
        Some(0) | Some(200) if status < 400 => Ok(take_data(v)),
        Some(c) => {
            if c == 429
                && let Some(s) = r.retry_after_s
            {
                pause(s);
            }
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or_default();
            // Решение 263: со статусом — код не из списка при HTTP 5XX не отказ, а сбой MEXC (errors.rs).
            Err(errors::from_code_http(c, msg, op, &r.text, Some(status)))
        }
        None if status < 400 => Ok(v),
        None => Err(errors::from_http(status, &r.text, r.retry_after_s)),
    }
}

/// Поле data ответа (как `v.get("data").cloned().unwrap_or(v)`, но без копии): есть — оно, нет — весь ответ.
/// Ошибок нет.
fn take_data(mut v: Value) -> Value {
    if let Some(d) = v.as_object_mut().and_then(|m| m.remove("data")) {
        return d;
    }
    v
}
