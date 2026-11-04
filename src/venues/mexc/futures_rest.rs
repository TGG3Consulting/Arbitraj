//! Файл: venues/mexc/futures_rest.rs
//! Что делает: подписанный GET к фьючерсному API MEXC (свой домен и своя подпись, не как у спота) — только чтение,
//! для панели «Балансы» (решение 144). Ответ пишется в сырой журнал (raw.*) — для разбора и тестов.
//! Кто вызывает: all_balances.rs.
//! От чего зависит: ctx.rs (настройки, ключи, HTTP, журнал), errors.rs, gateway/http.rs, hmac, sha2.
//! Настройки: venue.futures_rest_url [mexc] (адрес фьючерсного REST; по документации — https://contract.mexc.com),
//! http.*; секреты mexc.api_key, mexc.api_secret (те же ключи, что у спота).
//!
//! Официальная документация фьючерсов (Contract API v1): раздел фьючерсов https://www.mexc.com/api-docs/futures/
//! (сам сайт из среды разработки недоступен — сверено по официальному исходнику документации MEXC
//! https://github.com/mexcdevelop/apidocs, ветка contract_v1_en, source/index.html.md, коммит от 2025-11-20):
//! - «Access to url»: базовый адрес https://contract.mexc.com; ответ {success, code, data} или
//!   {success:false, code, message}.
//! - «Authentication method»: заголовки ApiKey, Request-Time (мс), Signature, Content-Type application/json;
//!   строка подписи = accessKey + timestamp + строка параметров (у GET — параметры по алфавиту через &, без параметров
//!   — ""); HMAC SHA256, hex без base64. Recv-Window необязателен (по умолчанию окно 10 с, «Time security») — не шлём.
//! - «Error code»: 401 Unauthorized, 402 Api_key expired, 406 IP не в белом списке, 510 слишком частые запросы,
//!   513 время запроса вне окна, 602 подпись не прошла, 701/703 — нет права чтения счёта / торговой информации,
//!   1000 счёт не существует.
//!
//! Retry-After у фьючерсов в документации не описан; если он пришёл с 418/429 — пауза своя (не общая со спотом:
//! другой домен, лимит фьючерсов не должен останавливать спот-торговлю).

use std::sync::atomic::{AtomicI64, Ordering};

use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::{Ctx, ID};
use super::errors;

/// Имя настройки адреса фьючерсного REST (область — площадка mexc).
pub const URL_SETTING: &str = "venue.futures_rest_url";

/// До какого момента (unix мс) фьючерсные запросы не шлются — по Retry-After после 418/429. 0 — паузы нет.
static PAUSE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// Что делает: подписанный GET без параметров к фьючерсному API.
/// Принимает: окружение, путь (например «/api/v1/private/account/assets»).
/// Возвращает: поле data успешного ответа.
/// Ошибки: NotConfigured (нет venue.futures_rest_url или ключей), SecretsLocked, RateLimited (пауза, 418/429, код
/// 510), InsufficientPermissions (401, 701, 703), InvalidCredentials (402, 602), IpNotAllowed (406), BadResponse.
pub async fn get(ctx: &Ctx, path: &str) -> GatewayResult<Value> {
    let base = ctx.text(URL_SETTING).map_err(|_| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("не задана {URL_SETTING} — счёт не читается"),
        ))
    })?;
    let left = PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::RateLimited,
            None,
            format!("MEXC фьючерсы: пауза по Retry-After — ещё {left} мс запросы не отправляются"),
        )));
    }
    let (key, secret) = ctx.keys()?;
    let ts = chrono::Utc::now().timestamp_millis().to_string();
    // Строка подписи: accessKey + timestamp + "" (у запроса нет параметров) — «Authentication method», п. 1–2.
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Пустой секретный ключ MEXC",
        ))
    })?;
    mac.update(key.as_bytes());
    mac.update(ts.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    let url = format!("{}{path}", base.trim_end_matches('/'));
    let req = ctx
        .http
        .get(url)
        .header("ApiKey", key.as_str())
        .header("Request-Time", ts)
        .header("Signature", signature)
        .header("Content-Type", "application/json");
    let lim = ctx.limits()?;
    let r = crate::gateway::http::exchange_full(req, lim)
        .await
        .map_err(|f| errors::from_fail(&f, &lim))?;
    ctx.raw.write(ID, &format!("rest futures {path}"), &r.text);
    let status = r.status.as_u16();
    if matches!(status, 418 | 429) {
        if let Some(s) = r.retry_after_s {
            let ms = i64::try_from(s.saturating_mul(1000)).unwrap_or(i64::MAX);
            let until = chrono::Utc::now().timestamp_millis().saturating_add(ms);
            PAUSE_UNTIL_MS.fetch_max(until, Ordering::Relaxed);
        }
        return Err(fail(
            ctx,
            ErrorCode::RateLimited,
            format!("HTTP {status}"),
            &r.text,
        ));
    }
    let v: Value = serde_json::from_str(&r.text).map_err(|_| {
        fail(
            ctx,
            ErrorCode::BadResponse,
            format!("HTTP {status}, тело не JSON"),
            &r.text,
        )
    })?;
    let code = v.get("code").and_then(Value::as_i64);
    if v.get("success").and_then(Value::as_bool) == Some(true) && code == Some(0) {
        return v
            .get("data")
            .cloned()
            .ok_or_else(|| fail(ctx, ErrorCode::BadResponse, "нет поля data".into(), &r.text));
    }
    let msg = v
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("(без message)");
    let Some(c) = code else {
        return Err(fail(
            ctx,
            ErrorCode::BadResponse,
            format!("HTTP {status}: {msg}"),
            &r.text,
        ));
    };
    Err(fail(
        ctx,
        unified(c),
        format!("код {c}{}: {msg}", explain(c)),
        &r.text,
    ))
}

/// Единый код по коду ошибки фьючерсного API («Error code»). Принимает: код. Возвращает: ErrorCode. Ошибок нет.
fn unified(code: i64) -> ErrorCode {
    match code {
        401 | 701 | 703 => ErrorCode::InsufficientPermissions,
        402 | 602 => ErrorCode::InvalidCredentials,
        406 => ErrorCode::IpNotAllowed,
        510 => ErrorCode::RateLimited,
        500 | 501 => ErrorCode::Unreachable,
        _ => ErrorCode::BadResponse,
    }
}

/// Пояснение по-русски к коду из таблицы «Error code» (перевод описания документации). Пусто — кода нет в списке.
fn explain(code: i64) -> &'static str {
    match code {
        401 => " (Unauthorized — ключ не допущен к фьючерсному API)",
        402 => " (срок ключа истёк)",
        406 => " (IP не в белом списке ключа)",
        510 => " (слишком частые запросы)",
        513 => " (время запроса вне окна 10 с — проверьте часы)",
        602 => " (подпись не прошла)",
        701 => " (нужно право чтения счёта у ключа)",
        703 => " (нужно право чтения торговой информации у ключа)",
        1000 => " (фьючерсный счёт не существует — фьючерсы не открыты)",
        _ => "",
    }
}

/// Ошибка с площадкой MEXC и сырым ответом. Принимает: окружение, код, текст, сырой ответ. Возвращает: GatewayError.
fn fail(ctx: &Ctx, code: ErrorCode, text: String, raw: &str) -> GatewayError {
    ctx.tag(GatewayError::new(code, None, format!("MEXC фьючерсы: {text}")).with_raw(raw))
}
