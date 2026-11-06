//! Файл: venues/bitget/reply.rs
//! Что делает: разбор ответа REST Bitget (код HTTP, тело {code, msg, requestTime, data}) в значение или единую ошибку
//! шлюза и пауза запросов после ответа «слишком часто» (HTTP 429 / код 429) — до её конца адаптер сам не шлёт запросов
//! к этому эндпоинту (урок 15). Решение 201: пауза — по эндпоинту (путь без параметров), а не на всю биржу: по
//! документации у каждого эндпоинта свой предел, считаются независимо; раньше 429 справочника монет (3/с по IP)
//! останавливал и баланс, и ордер, и «Продать сейчас». Ответы пишутся в сырой журнал (raw.*).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [bitget] — пауза после 429, если Bitget не прислал Retry-After.
//!
//! Официально (Classic → REST API, https://www.bitget.com/docs/classic/rest-api): «If the rate limit is exceeded, a
//! 429 status code will be returned»; общий предел «6000/IP/Min», у каждого эндпоинта — свой (по UID или IP). Заголовка
//! Retry-After и времени до сброса окна в документации нет → пауза venue.rate_limit_pause_ms (как Gate); бан IP не
//! описан. Успех — «"code": "00000"» с полем data (примеры всех эндпоинтов Classic v2).

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// Код успеха Bitget — из официальных примеров ответов («"code": "00000"»).
pub const OK_CODE: &str = "00000";

/// Паузы по эндпоинтам: путь → до какого момента (unix мс) не слать. Один адаптер Bitget на процесс.
static PAUSE: LazyLock<RwLock<HashMap<String, i64>>> = LazyLock::new(Default::default);

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Что делает: остаток паузы эндпоинта, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201,
/// gateway/patience.rs). Принимает: путь. Ошибок нет.
pub fn pause_left_ms(path: &str) -> i64 {
    PAUSE
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(path)
        .copied()
        .unwrap_or(0)
        - now_ms()
}

/// Что делает: не пускает запрос, пока эндпоинт на паузе после 429.
/// Принимает: путь. Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate(path: &str) -> GatewayResult<()> {
    let left = pause_left_ms(path);
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!("Bitget {path}: пауза после HTTP 429 — ещё {left} мс запросы не отправляются"),
        ));
    }
    Ok(())
}

/// Что делает: ставит паузу эндпоинта на ms миллисекунд от текущего момента (дальняя из старой и новой). Ошибок нет.
fn pause_ms(path: &str, ms: u64) {
    let ms = i64::try_from(ms).unwrap_or(i64::MAX);
    let until = now_ms().saturating_add(ms);
    let mut m = PAUSE.write().unwrap_or_else(|p| p.into_inner());
    let e = m.entry(path.to_string()).or_insert(0);
    *e = (*e).max(until);
}

/// Что делает: пауза после 429 — Retry-After (секунды), иначе venue.rate_limit_pause_ms. Не задана и Retry-After нет —
/// паузы нет, в тексте ошибки сказано, что настройка не задана. Возвращает: пояснение для текста ошибки. Ошибок нет.
fn on_limit(ctx: &Ctx, path: &str, retry_after_s: Option<u64>) -> String {
    if let Some(s) = retry_after_s {
        pause_ms(path, s.saturating_mul(1000));
        return format!("пауза {s} с по Retry-After");
    }
    match ctx.int("venue.rate_limit_pause_ms") {
        Ok(ms) => {
            pause_ms(path, ms);
            format!("пауза {ms} мс (venue.rate_limit_pause_ms)")
        }
        Err(_) => "venue.rate_limit_pause_ms не задана — паузы нет, задайте её".into(),
    }
}

/// Что делает: разбор ответа.
/// Принимает: окружение, путь (для журнала), ответ транспорта, операцию.
/// Возвращает: поле data ответа (забирается без копии).
/// Ошибки: RateLimited (429 — пауза), коды Bitget (errors.rs), BadResponse — тело не JSON.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    let v = serde_json::from_str::<Value>(&r.text).ok();
    let code = v.as_ref().and_then(|v| v.get("code")).and_then(code_text);
    if status < 400
        && code.as_deref() == Some(OK_CODE)
        && let Some(mut v) = v
    {
        return Ok(v
            .as_object_mut()
            .and_then(|m| m.remove("data"))
            .unwrap_or(Value::Null));
    }
    let limited = status == 429 || code.as_deref() == Some("429");
    let note = limited.then(|| on_limit(ctx, path, r.retry_after_s));
    let mut e = match (&v, code.as_deref()) {
        (Some(v), Some(c)) => {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or_default();
            errors::from_code(status, c, msg, op, &r.text)
        }
        // 2xx без понятного code — тело не то, что описано в документации.
        _ if status < 400 => errors::bad(&format!("{path}: нет code \"00000\""), &r.text),
        _ => errors::from_http(status, op, &r.text),
    };
    if let Some(n) = note {
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
