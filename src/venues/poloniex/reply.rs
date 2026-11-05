//! Файл: venues/poloniex/reply.rs
//! Что делает: разбор ответа REST Poloniex (код HTTP и тело) в значение или единую ошибку шлюза и пауза запросов после
//! ответа «слишком часто» (HTTP 429) — до её конца адаптер сам не шлёт ни одного запроса (урок 15). Ответы пишутся в
//! сырой журнал (raw.*).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [poloniex] — пауза после 429, если Poloniex не прислал Retry-After.
//!
//! Официально: у успешного ответа обёртки нет — тело и есть данные (примеры всех эндпоинтов Spot API: массив или
//! объект). Формат ошибки в документации не описан (страница Error Codes — только коды и тексты); официальный SDK
//! (polosdk/spot/rest/request.py) считает ошибкой любой HTTP-статус, кроме 200, и берёт из тела поля code и message —
//! так и здесь (2xx — успех; иначе code и message, проверить на живом ответе — урок 2). Ответ при превышении лимита
//! (Spot → API → Rate Limits) в документации не описан: 429 — по общему смыслу HTTP, Retry-After не описан → пауза
//! venue.rate_limit_pause_ms (как Gate и Bitget); бан IP не описан.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// До какого момента (unix мс) запросы к Poloniex не отправляются. Один адаптер Poloniex на процесс. 0 — паузы нет.
static PAUSE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// Что делает: не пускает запрос, пока идёт пауза после 429.
/// Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate() -> GatewayResult<()> {
    let left = PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!("Poloniex: пауза после HTTP 429 — ещё {left} мс запросы не отправляются"),
        ));
    }
    Ok(())
}

/// Что делает: ставит паузу на ms миллисекунд от текущего момента (дальняя из старой и новой). Ошибок нет.
fn pause_ms(ms: u64) {
    let ms = i64::try_from(ms).unwrap_or(i64::MAX);
    let until = chrono::Utc::now().timestamp_millis().saturating_add(ms);
    PAUSE_UNTIL_MS.fetch_max(until, Ordering::Relaxed);
}

/// Что делает: пауза после 429 — Retry-After (секунды), иначе venue.rate_limit_pause_ms. Не задана и Retry-After нет —
/// паузы нет, в тексте ошибки сказано, что настройка не задана. Возвращает: пояснение для текста ошибки. Ошибок нет.
fn on_limit(ctx: &Ctx, retry_after_s: Option<u64>) -> String {
    if let Some(s) = retry_after_s {
        pause_ms(s.saturating_mul(1000));
        return format!("пауза {s} с по Retry-After");
    }
    match ctx.int("venue.rate_limit_pause_ms") {
        Ok(ms) => {
            pause_ms(ms);
            format!("пауза {ms} мс (venue.rate_limit_pause_ms)")
        }
        Err(_) => "venue.rate_limit_pause_ms не задана — паузы нет, задайте её".into(),
    }
}

/// Что делает: разбор ответа.
/// Принимает: окружение, путь (для журнала), ответ транспорта, операцию.
/// Возвращает: тело ответа (JSON).
/// Ошибки: RateLimited (429 — пауза), коды Poloniex (errors.rs), BadResponse — тело не JSON.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    let v = serde_json::from_str::<Value>(&r.text).ok();
    if (200..300).contains(&status) {
        return v.ok_or_else(|| errors::bad(&format!("{path}: тело не JSON"), &r.text));
    }
    let note = (status == 429).then(|| on_limit(ctx, r.retry_after_s));
    let code = v.as_ref().and_then(|v| v.get("code")).and_then(code_text);
    let mut e = match (&v, code.as_deref()) {
        (Some(v), Some(c)) => {
            let msg = v.get("message").and_then(Value::as_str).unwrap_or_default();
            errors::from_code(status, c, msg, op, &r.text)
        }
        _ => errors::from_http(status, op, &r.text),
    };
    if let Some(n) = note {
        e.code = ErrorCode::RateLimited;
        e.message = format!("{} — {n}", e.message);
    }
    Err(e)
}

/// Поле code — строка или число JSON → текст. Ошибок нет.
fn code_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
