//! Файл: venues/htx/reply.rs
//! Что делает: разбор ответа REST HTX в значение data или единую ошибку шлюза — два формата ответа (v1 и v2) — и пауза
//! запросов после ответа «слишком часто» (HTTP 429): до её конца адаптер сам не шлёт ни одного запроса (урок 15).
//! Ответы пишутся в сырой журнал (raw.*).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [htx] — пауза после 429, если HTX не прислал Retry-After.
//!
//! Официально («API Access → Response Format», https://huobiapi.github.io/docs/spot/v1/en/): v1 — {status: "ok" |
//! "error", err-code, err-msg, data}; эндпоинты /v1/settings/common/* и /v2/settings/* — {status, data, ts, full,
//! err_code, err_msg} (ошибка через «_»); v2 (/v2/reference/*, /v2/account/*, /v2/user/*) — {code: 200 — успех,
//! message, data}. Ответ на превышение лимита REST и Retry-After в документации не описаны (есть только заголовки
//! X-HB-RateLimit-Requests-Remain / -Expire — транспорт шлюза заголовков не отдаёт): пауза — по HTTP 429 на
//! venue.rate_limit_pause_ms; что HTX присылает на самом деле — проверить на живом ответе.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// Код успеха ответа v2 — «code: 200» («Wallet → Error Code»: 200 success).
pub const V2_OK: i64 = 200;

/// До какого момента (unix мс) запросы к HTX не отправляются. Один адаптер HTX на процесс. 0 — паузы нет.
static PAUSE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// Что делает: остаток паузы после 429, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201,
/// gateway/patience.rs). Ошибок нет.
pub fn pause_left_ms() -> i64 {
    PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis()
}

/// Что делает: не пускает запрос, пока идёт пауза после 429.
/// Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate() -> GatewayResult<()> {
    let left = PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!("HTX: пауза после HTTP 429 — ещё {left} мс запросы не отправляются"),
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
/// паузы нет, в тексте ошибки сказано. Возвращает: пояснение для текста ошибки. Ошибок нет.
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

/// Что делает: разбор ответа. Принимает: окружение, путь (для журнала), ответ транспорта, операцию.
/// Возвращает: поле data (забирается без копии); нет data — поле tick (рыночные эндпоинты: «Get Market Depth» отвечает
/// {ch, status, ts, tick}); нет и его — null.
/// Ошибки: RateLimited (429 — пауза), коды HTX (errors.rs), BadResponse — тело не JSON или без признака успеха.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    let mut v = serde_json::from_str::<Value>(&r.text).ok();
    if status < 400 && v.as_ref().is_some_and(success) {
        return Ok(v
            .as_mut()
            .and_then(Value::as_object_mut)
            .and_then(|m| m.remove("data").or_else(|| m.remove("tick")))
            .unwrap_or(Value::Null));
    }
    let note = (status == 429).then(|| on_limit(ctx, r.retry_after_s));
    let mut e = match v.as_ref().and_then(code_msg) {
        Some((c, m)) => errors::from_code(status, &c, &m, op, &r.text),
        None if status < 400 => errors::bad(&format!("{path}: нет признака успеха"), &r.text),
        None => errors::from_http(status, op, &r.text),
    };
    if let Some(n) = note {
        e.message = format!("{} — {n}", e.message);
    }
    Err(e)
}

/// Успешный ответ: status = "ok" (v1) или code = 200 (v2). Ошибок нет.
pub fn success(v: &Value) -> bool {
    match (v.get("status"), v.get("code")) {
        (Some(Value::String(s)), _) => s == "ok",
        (_, Some(c)) => c.as_i64() == Some(V2_OK) || c.as_str() == Some("200"),
        _ => false,
    }
}

/// Код и сообщение ошибки: err-code / err-msg (v1), err_code / err_msg (settings), code / message (v2). Ошибок нет.
pub fn code_msg(v: &Value) -> Option<(String, String)> {
    let text = |k: &str| match v.get(k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let code = text("err-code")
        .or_else(|| text("err_code"))
        .or_else(|| text("code"))?;
    let msg = text("err-msg")
        .or_else(|| text("err_msg"))
        .or_else(|| text("message"))
        .unwrap_or_default();
    Some((code, msg))
}
