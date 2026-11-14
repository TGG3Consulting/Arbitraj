//! Файл: venues/binance/reply.rs
//! Что делает: разбор ответа REST Binance (код HTTP, тело, Retry-After) в значение или единую ошибку шлюза и пауза
//! запросов после 418/429 — до её конца адаптер сам не шлёт ни одного запроса этого семейства (/api или /sapi).
//! Ответы пишутся в сырой журнал (raw.*).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [binance] — пауза после 429, если Binance не прислал Retry-After.
//!
//! Официально:
//! - rest-api «LIMITS» https://developers.binance.com/legacy-docs/binance-spot-api-docs/rest-api — лимиты /api/* по IP;
//!   «When a 429 is received, it's your obligation as an API to back off», после 429 — бан IP с HTTP 418 «from 2 minutes
//!   to 3 days»; «A Retry-After header is sent with a 418 or 429 responses and will give the number of seconds required
//!   to wait». Поэтому 418/429 — всегда RateLimited по статусу, и пауза не короче Retry-After.
//! - Wallet «General Info» (SAPI): у каждого эндпоинта /sapi свой предел (IP 12000/мин, UID 180000/мин), при
//!   превышении — 429, бан 418 к SAPI не применяется; про Retry-After у SAPI прямо не сказано — тогда пауза
//!   venue.rate_limit_pause_ms. Пауза /sapi не останавливает /api (семейства независимы).
//! - Ошибка — {"code": отрицательное, "msg"} (errors.md). Успех — объект или массив без отрицательного code.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// До какого момента (unix мс) запросы /api/* не отправляются. Один адаптер Binance на процесс. 0 — паузы нет.
static PAUSE_API_MS: AtomicI64 = AtomicI64::new(0);
/// То же для /sapi/* (свои пределы по эндпоинту, Wallet «General Info»).
static PAUSE_SAPI_MS: AtomicI64 = AtomicI64::new(0);

/// Пауза семейства пути. Ошибок нет.
fn slot(path: &str) -> &'static AtomicI64 {
    if path.starts_with("/sapi/") {
        &PAUSE_SAPI_MS
    } else {
        &PAUSE_API_MS
    }
}

/// Что делает: остаток паузы, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201, gateway/patience.rs).
/// Принимает: то же, что gate. Ошибок нет.
pub fn pause_left_ms(path: &str) -> i64 {
    slot(path).load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis()
}

/// Что делает: не пускает запрос, пока идёт пауза его семейства.
/// Принимает: путь. Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate(path: &str) -> GatewayResult<()> {
    let left = slot(path).load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!(
                "Binance: пауза после HTTP 418/429 — ещё {left} мс запросы {path} не отправляются"
            ),
        ));
    }
    Ok(())
}

/// Что делает: пауза после 418/429 — Retry-After (секунды), иначе venue.rate_limit_pause_ms. Не задана и Retry-After
/// нет — паузы нет (сказано в тексте ошибки). Принимает: окружение, путь, Retry-After. Возвращает: пояснение.
/// Ошибок нет.
fn on_limit(ctx: &Ctx, path: &str, retry_after_s: Option<u64>) -> String {
    let ms = match retry_after_s {
        Some(s) => s.saturating_mul(1000),
        None => match ctx.int("venue.rate_limit_pause_ms") {
            Ok(ms) => ms,
            Err(_) => return "venue.rate_limit_pause_ms не задана — паузы нет, задайте её".into(),
        },
    };
    let until = chrono::Utc::now()
        .timestamp_millis()
        .saturating_add(i64::try_from(ms).unwrap_or(i64::MAX));
    slot(path).fetch_max(until, Ordering::Relaxed);
    format!("запросы не отправляются {ms} мс")
}

/// Что делает: разбор ответа.
/// Принимает: окружение, путь (для журнала и паузы), ответ транспорта, операцию.
/// Возвращает: тело JSON. Ошибки: RateLimited (418/429, −1003), InsufficientPermissions (403 — WAF), коды Binance
/// (errors.rs), BadResponse — тело не JSON.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    if matches!(status, 418 | 429) {
        let pause = on_limit(ctx, path, r.retry_after_s);
        return Err(errors::from_http(status, &r.text, r.retry_after_s, &pause));
    }
    let Ok(v) = serde_json::from_str::<Value>(&r.text) else {
        return Err(errors::from_http(status, &r.text, r.retry_after_s, ""));
    };
    match v.get("code").and_then(Value::as_i64) {
        Some(c) if c < 0 || status >= 400 => {
            if c == -1003 {
                on_limit(ctx, path, r.retry_after_s);
            }
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or_default();
            // Дополнение 268 (ревью 2): при HTTP 5XX код PriceOutOfRange не присваивается (как до решения 268 — OrderRejected):
            // по ответу с ошибкой сервера рыночный ордер не ставится.
            let e = errors::from_code(c, msg, op, &r.text);
            Err(if status >= 500 { e.no_price_range() } else { e })
        }
        _ if status >= 400 => Err(errors::from_http(status, &r.text, r.retry_after_s, "")),
        _ => Ok(v),
    }
}
