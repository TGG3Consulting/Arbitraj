//! Файл: venues/kraken/reply.rs
//! Что делает: разбор ответа REST Kraken в значение или единую ошибку шлюза и паузы после «слишком часто» (урок 15: не
//! слать дальше). Спот REST — {"error":[…],"result":…}; Funding (Beta) — объект ответа как есть при 2xx, иначе ошибка.
//! Ответы пишутся в сырой журнал (тела ответов Kraken секретов не содержат).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [kraken] — пауза после «Rate limit exceeded» (у Kraken Retry-After нет).
//!
//! Официально ([RL-REST], [RL-TRADE] — docs/notes/kraken-analysis-2026-11-05.md): счётчик REST — на ключ (все
//! приватные вызовы), «EAPI:Rate limit exceeded» — ограничение «a few seconds», дольше, если продолжать слать;
//! «EService: Throttled: [UNIX timestamp]» — ждать до метки; счётчик торгового движка — на пару («EOrder:Rate limit
//! exceeded»). Поэтому паузы три: «private» (все подписанные), «trade» (постановка ордеров) и путь публичного запроса.
//! Лимиты Funding (Beta) не описаны — его запросы идут под паузой «private» (тот же ключ).

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// Пауза всех подписанных запросов ключа.
pub const PRIVATE: &str = "private";
/// Пауза постановки ордеров (счётчик торгового движка).
pub const TRADE: &str = "trade";

/// До какого момента (unix мс) закрыт ключ паузы. Один адаптер Kraken на процесс — состояние модуля.
static PAUSE: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Что делает: не пускает запрос, пока ключ паузы закрыт. Принимает: ключ паузы. Возвращает: Ok — можно слать.
/// Ошибки: RateLimited — сколько ещё ждать.
pub fn gate(key: &str) -> GatewayResult<()> {
    let until = PAUSE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(key).copied())
        .unwrap_or(0);
    let left = until - now_ms();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!(
                "Kraken: {key} на паузе после «слишком часто» — ещё {left} мс запросы не отправляются"
            ),
        ));
    }
    Ok(())
}

/// Закрывает ключ паузы до момента until (unix мс). Ошибок нет.
fn close(key: &str, until: i64) {
    let mut g = PAUSE.lock().unwrap_or_else(|p| p.into_inner());
    let e = g
        .get_or_insert_with(HashMap::new)
        .entry(key.to_string())
        .or_insert(0);
    *e = (*e).max(until);
}

/// Ставит паузу по строке ошибки: Throttled — до метки из текста; прочее — venue.rate_limit_pause_ms на ключ паузы.
/// Возвращает: пояснение для текста ошибки.
fn pause(ctx: &Ctx, key: &str, err: &str) -> String {
    if let Some(ts) = throttled_until(err) {
        close(PRIVATE, ts * 1000);
        return format!(" — запросы ключа на паузе до {ts} (метка Kraken)");
    }
    let key = if err.replace(' ', "").starts_with("EOrder:") {
        TRADE
    } else {
        key
    };
    match ctx.int("venue.rate_limit_pause_ms") {
        Ok(ms) => {
            close(key, now_ms() + ms as i64);
            format!(" — {key} на паузе {ms} мс")
        }
        Err(_) => " — пауза не задана (venue.rate_limit_pause_ms)".into(),
    }
}

/// Метка из «EService: Throttled: [UNIX timestamp]» (секунды). None — не Throttled или метки нет. Ошибок нет.
pub fn throttled_until(err: &str) -> Option<i64> {
    let s = err.replace(' ', "");
    let rest = s.strip_prefix("EService:Throttled:")?;
    rest.trim_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()
}

/// Что делает: разбор ответа спот REST. Принимает: окружение, путь, ключ паузы, ответ, операцию.
/// Возвращает: поле result (забирается без копии). Ошибки: строки error[] (errors.rs), RateLimited (пауза), BadResponse.
pub fn parse_spot(ctx: &Ctx, path: &str, key: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    let Ok(mut v) = serde_json::from_str::<Value>(&r.text) else {
        let mut e = errors::from_http(status, &r.text);
        if status == 429 {
            e.message += &pause(ctx, key, "");
        }
        return Err(e);
    };
    let list: Vec<String> = v
        .get("error")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !list.is_empty() {
        let mut e = errors::from_errors(&list, op, &r.text);
        if let Some(rl) = list.iter().find(|x| errors::is_rate_limit(x)) {
            e.code = ErrorCode::RateLimited;
            e.message += &pause(ctx, key, rl);
        }
        return Err(e);
    }
    match v.as_object_mut().and_then(|m| m.remove("result")) {
        Some(res) if status < 400 => Ok(res),
        _ => Err(errors::from_http(status, &r.text)),
    }
}

/// Что делает: разбор ответа Funding (Beta). Принимает: окружение, путь, ответ, операцию.
/// Возвращает: тело JSON (пустое тело — Null). Ошибки: не 2xx (errors::from_funding; 429 — пауза), BadResponse.
pub fn parse_funding(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    if !(200..300).contains(&status) {
        let mut e = errors::from_funding(status, &r.text, op);
        if status == 429 {
            e.message += &pause(ctx, PRIVATE, "");
        }
        return Err(e);
    }
    if r.text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&r.text)
        .map_err(|_| errors::bad(&format!("{path}: тело не JSON"), &r.text))
}
