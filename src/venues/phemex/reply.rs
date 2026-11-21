//! Файл: venues/phemex/reply.rs
//! Что делает: разбор ответа REST Phemex (код HTTP; тело {code, msg, data}, у путей /md… — {error, id, result}) в
//! значение или единую ошибку шлюза и пауза запросов после «слишком часто» (HTTP 429) — до её конца адаптер сам не
//! шлёт ни одного запроса (урок 15; у Phemex превышение IP-предела — блок IP на 5 минут). Ответы — в сырой журнал.
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [phemex] — пауза после 429, если не пришёл Retry-After.
//!
//! Официально (General API information, https://phemex-docs.github.io/#rest-response-format и #rate-limits):
//! - «All restful API except starting with /md shares same response format» {code, msg, data}, «code: 0 means
//!   success»; ответы /md… — {"error": null, "id": 0, "result": …} (Query order book).
//! - «If exceeded the ratelimit, http status 429 will returned, together with a reset seconds header
//!   x-ratelimit-retry-after-<groupName>»; группы Contract, SpotOrder (500/мин), Others (100/мин); IP — «5,000 requests
//!   in 5 minutes window. If exceeded … blocked in the following 5 minutes». Транспорт шлюза (gateway/http.rs) читает
//!   только стандартный Retry-After — заголовок группы Phemex не читается, поэтому пауза venue.rate_limit_pause_ms
//!   (решает Тигран; запись в отчёте «проверить на живом ответе 429»).

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// До какого момента (unix мс) запросы к Phemex не отправляются. Один адаптер Phemex на процесс. 0 — паузы нет.
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
            crate::gateway::errors::ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!("Phemex: пауза после HTTP 429 — ещё {left} мс запросы не отправляются"),
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
/// Принимает: окружение, путь (для журнала и вида ответа), ответ транспорта, операцию.
/// Возвращает: data (или result у /md…), забирается без копии.
/// Ошибки: RateLimited (429 — пауза), коды Phemex (errors.rs), BadResponse — тело не JSON или без code.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    let mut v = serde_json::from_str::<Value>(&r.text).ok();
    let md = path.starts_with("/md");
    if status < 400
        && let Some(x) = v.as_mut()
    {
        if md && x.get("error").is_some_and(Value::is_null) {
            return Ok(take(x, "result"));
        }
        if !md && x.get("code").and_then(code_text).as_deref() == Some("0") {
            return Ok(take(x, "data"));
        }
    }
    let note = (status == 429).then(|| on_limit(ctx, r.retry_after_s));
    let mut e = match &v {
        Some(v) if !md && v.get("code").is_some() => {
            let code = v.get("code").and_then(code_text).unwrap_or_default();
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or_default();
            errors::from_code(status, &code, msg, op, &r.text)
        }
        Some(v) if md && v.get("error").is_some_and(|e| !e.is_null()) => {
            let text = v.get("error").map(Value::to_string).unwrap_or_default();
            errors::from_code(status, "error", &text, op, &r.text)
        }
        // 2xx без понятного code — тело не то, что описано в документации.
        _ if status < 400 => errors::bad(&format!("{path}: нет code 0 / error null"), &r.text),
        _ => errors::from_http(status, op, &r.text),
    };
    if let Some(n) = note {
        e.message = format!("{} — {n}", e.message);
    }
    Err(e)
}

/// Забирает поле из объекта без копии (нет — null). Ошибок нет.
fn take(v: &mut Value, field: &str) -> Value {
    v.as_object_mut()
        .and_then(|m| m.remove(field))
        .unwrap_or(Value::Null)
}

/// Поле code — число или строка JSON → текст. Ошибок нет.
fn code_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
