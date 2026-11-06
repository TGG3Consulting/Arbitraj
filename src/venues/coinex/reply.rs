//! Файл: venues/coinex/reply.rs
//! Что делает: разбор ответа REST CoinEx (HTTP-статус и тело {code, data, message[, pagination]}) в тело или единую
//! ошибку шлюза и пауза запросов после «слишком часто» (коды 4213, 3008, 4001; HTTP 429) — до её конца адаптер сам не шлёт
//! ни одного запроса (урок 15). Ответы пишутся в сырой журнал (raw.*).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [coinex] — пауза после лимита, если CoinEx не прислал Retry-After.
//!
//! Официально:
//! - Integration Guide (https://docs.coinex.com/api/v2/guide): «check that the status code is 200 first»; тело — «code
//!   (0 when normal), data, and message»; Error Handling: не 200 — ответ шлюза, тело не разбирать как JSON.
//! - Rate Limit (https://docs.coinex.com/api/v2/rate-limit): после лимита — 3008, 4001, 4213; заголовки
//!   X-RateLimit-Limit / X-RateLimit-Remaining (транспорт шлюза их не отдаёт — не читаются); HTTP-статус лимита и
//!   Retry-After не описаны → пауза venue.rate_limit_pause_ms (как Gate и Bitget); «low-speed mode» длинного цикла — те
//!   же коды.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, LIMIT_CODES, Op};

/// До какого момента (unix мс) запросы к CoinEx не отправляются. Один адаптер CoinEx на процесс. 0 — паузы нет.
static PAUSE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// Что делает: остаток паузы после 429, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201,
/// gateway/patience.rs). Ошибок нет.
pub fn pause_left_ms() -> i64 {
    PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis()
}

/// Что делает: не пускает запрос, пока идёт пауза после лимита.
/// Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate() -> GatewayResult<()> {
    let left = PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            crate::gateway::errors::ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!("CoinEx: пауза после лимита запросов — ещё {left} мс запросы не отправляются"),
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

/// Что делает: пауза после лимита — Retry-After (секунды), иначе venue.rate_limit_pause_ms. Не задана и Retry-After нет —
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
/// Возвращает: всё тело ответа (data, pagination — у вызывающего, rest.rs).
/// Ошибки: RateLimited (пауза), коды CoinEx (errors.rs), HTTP не 200 (ответ шлюза), BadResponse — тело не JSON / нет code.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    if status != 200 {
        let mut e = errors::from_http(status, op, &r.text);
        if status == 429 {
            e.message = format!("{} — {}", e.message, on_limit(ctx, r.retry_after_s));
        }
        return Err(e);
    }
    let v = serde_json::from_str::<Value>(&r.text)
        .map_err(|_| errors::bad(&format!("{path}: тело не JSON"), &r.text))?;
    let code = v
        .get("code")
        .and_then(code_text)
        .ok_or_else(|| errors::bad(&format!("{path}: нет code"), &r.text))?;
    if code == "0" {
        return Ok(v);
    }
    let msg = v.get("message").and_then(Value::as_str).unwrap_or_default();
    let mut e = errors::from_code(&code, msg, op, &r.text);
    if LIMIT_CODES.contains(&code.as_str()) {
        e.message = format!("{} — {}", e.message, on_limit(ctx, r.retry_after_s));
    }
    Err(e)
}

/// Поле code — число или строка JSON → текст. Ошибок нет.
fn code_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
