//! Файл: venues/okx/reply.rs
//! Что делает: разбор ответа REST OKX ({code, msg, data}; у ордеров и пакетных операций — ещё sCode/sMsg строки data) в
//! значение или единую ошибку шлюза; пауза эндпоинта после ответа «слишком часто» (не слать дальше — урок 15). Ответы
//! пишутся в сырой журнал (тела ответов OKX секретов не содержат).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [okx] — пауза, если площадка не прислала Retry-After.
//!
//! Официально (https://www.okx.com/docs-v5/en/): «Overview → REST API»: ответ — code, msg, data; «code and msg represent
//! the request result or error reason when the return data has code»; у Place order — «sCode … 0 means success», «sMsg
//! Rejection or success message of event execution». «Overview → Rate Limits»: лимиты — у каждого эндпоинта свои (по IP,
//! по User ID, по User ID + инструмент); «When a request is rejected by our system due to rate limits, the system returns
//! error code 50011» — «throttle requests accordingly». Retry-After в документации не описан — пауза настройкой.
//! Поэтому пауза ставится на путь эндпоинта, получившего отказ (лимиты независимы по эндпоинтам).

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// Код успеха OKX — «0» (таблица кодов: 0, HTTP 200).
pub const OK_CODE: &str = "0";

/// До какого момента (unix мс) путь закрыт после «слишком часто». Один адаптер OKX на процесс — состояние модуля.
static PAUSE: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Что делает: остаток паузы, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201, gateway/patience.rs).
/// Принимает: то же, что gate. Ошибок нет.
pub fn pause_left_ms(path: &str) -> i64 {
    PAUSE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(path).copied())
        .unwrap_or(0)
        - now_ms()
}

/// Что делает: не пускает запрос, пока путь на паузе после «слишком часто».
/// Принимает: путь эндпоинта. Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate(path: &str) -> GatewayResult<()> {
    let until = PAUSE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(path).copied())
        .unwrap_or(0);
    let left = until - now_ms();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!(
                "OKX: {path} на паузе после «слишком часто» — ещё {left} мс запросы не отправляются"
            ),
        ));
    }
    Ok(())
}

/// Ставит паузу пути: Retry-After ответа, иначе venue.rate_limit_pause_ms. Возвращает: пояснение для текста ошибки.
fn pause(ctx: &Ctx, path: &str, retry_after_s: Option<u64>) -> String {
    let ms = match retry_after_s {
        Some(s) => Some(s as i64 * 1000),
        None => ctx.int("venue.rate_limit_pause_ms").ok().map(|v| v as i64),
    };
    let Some(ms) = ms else {
        return " — пауза не задана (venue.rate_limit_pause_ms)".into();
    };
    let mut g = PAUSE.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let until = now_ms() + ms;
    let e = m.entry(path.to_string()).or_insert(0);
    *e = (*e).max(until);
    format!(" — {path} на паузе {ms} мс")
}

/// Что делает: разбор ответа.
/// Принимает: окружение, путь (для журнала и паузы), ответ, операцию.
/// Возвращает: поле data ответа (забирается без копии).
/// Ошибки: RateLimited (HTTP 429, 50011 и др. — путь на паузу), коды OKX (errors.rs), BadResponse — не JSON.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    let Ok(mut v) = serde_json::from_str::<Value>(&r.text) else {
        let mut e = errors::from_http(status, &r.text);
        if status == 429 {
            e.message += &pause(ctx, path, r.retry_after_s);
        }
        return Err(e);
    };
    let (code, msg) = outcome(&v);
    match code.as_deref() {
        Some(OK_CODE) if status < 400 => Ok(v
            .as_object_mut()
            .and_then(|m| m.remove("data"))
            .unwrap_or(Value::Null)),
        Some(c) => {
            let mut e = errors::from_code(c, &msg, op, &r.text);
            if errors::is_rate_limit(c) || status == 429 {
                e.code = ErrorCode::RateLimited;
                e.message += &pause(ctx, path, r.retry_after_s);
            }
            Err(e)
        }
        None => {
            let mut e = errors::from_http(status, &r.text);
            if status == 429 {
                e.message += &pause(ctx, path, r.retry_after_s);
            }
            Err(e)
        }
    }
}

/// Код и текст итога: верхний code/msg; если он не «0», а у первой строки data есть sCode не «0» — sCode/sMsg строки
/// (точная причина отказа ордера). Ошибок нет.
pub fn outcome(v: &Value) -> (Option<String>, String) {
    let top = v.get("code").and_then(code_text);
    let msg = v
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if top.as_deref() == Some(OK_CODE) {
        return (top, msg);
    }
    let row = v
        .get("data")
        .and_then(Value::as_array)
        .and_then(|a| a.first());
    match row.and_then(|r| r.get("sCode")).and_then(code_text) {
        Some(s) if s != OK_CODE => {
            let m = row
                .and_then(|r| r.get("sMsg"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            (Some(s), m.to_string())
        }
        _ => (top, msg),
    }
}

/// Поле code — строка или число JSON → текст. Ошибок нет.
fn code_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
