//! Файл: venues/xt/reply.rs
//! Что делает: разбор ответа REST XT.com (код HTTP и тело) в значение result или единую ошибку шлюза и пауза запросов
//! после 429 / риск-контроля — до её конца адаптер сам не шлёт ни одного запроса к этому адресу. Ответы пишутся в
//! сырой журнал (raw.*), кроме ответов с секретом ключа (запрос ключа user-center — урок 18).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [xt] — пауза после 429 (Retry-After в документации XT нет).
//!
//! Официально:
//! - формат https://doc.xt.com/docs/spot/Access%20Description/ResponseFormat (`_documentation/returnFormat.md`, errorCode.md):
//!   {"rc": 0, "mc": "SUCCESS", "ma": [], "result": {…}}; rc 1 — отказ, причина — mc; HTTP 200 — «please check the rc
//!   and mc sections further».
//! - лимиты https://doc.xt.com/docs/spot/Access%20Description/FrequencyLimitingRules: «If the interface request triggers the gateway flow limit, 429
//!   will be returned … and the IP or apiKey will be blocked» — после 429 ничего не слать. Время бана и Retry-After не
//!   описаны — пауза venue.rate_limit_pause_ms.
//! - фьючерсы (панель «Балансы») `_futures_documentation/returnFormat.md`: {"returnCode", "msgInfo", "error":
//!   {"code", "msg"}, "result"}; успех в примере returnCode 200, в примере getBalances — 0: успехом считаются оба,
//!   проверить на живом ответе.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// До какого момента (unix мс) запросы к споту sapi не отправляются. Один адаптер XT на процесс. 0 — паузы нет.
static PAUSE_SPOT_MS: AtomicI64 = AtomicI64::new(0);
/// То же для других адресов XT (фьючерсы fapi/dapi, user-center api.xt.com) — у них свои пределы.
static PAUSE_OTHER_MS: AtomicI64 = AtomicI64::new(0);

/// Пауза по виду адреса. Ошибок нет.
fn slot(spot: bool) -> &'static AtomicI64 {
    if spot {
        &PAUSE_SPOT_MS
    } else {
        &PAUSE_OTHER_MS
    }
}

/// Что делает: остаток паузы вида адреса, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201,
/// gateway/patience.rs). Принимает: спот ли. Ошибок нет.
pub fn pause_left_ms(spot: bool) -> i64 {
    slot(spot).load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis()
}

/// Что делает: не пускает запрос, пока идёт пауза. Принимает: спот ли, путь (для текста).
/// Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate(spot: bool, path: &str) -> GatewayResult<()> {
    let left = slot(spot).load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!(
                "XT: пауза после HTTP 429 / риск-контроля — ещё {left} мс запрос {path} не отправляется"
            ),
        ));
    }
    Ok(())
}

/// Что делает: ставит паузу venue.rate_limit_pause_ms. Не задана — паузы нет (сказано в тексте). Возвращает: пояснение.
/// Ошибок нет.
fn on_limit(ctx: &Ctx, spot: bool) -> String {
    let Ok(ms) = ctx.int("venue.rate_limit_pause_ms") else {
        return "venue.rate_limit_pause_ms не задана — паузы нет, задайте её".into();
    };
    let until = chrono::Utc::now()
        .timestamp_millis()
        .saturating_add(i64::try_from(ms).unwrap_or(i64::MAX));
    slot(spot).fetch_max(until, Ordering::Relaxed);
    format!("запросы не отправляются {ms} мс")
}

/// Как писать ответ в сырой журнал.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raw {
    /// Писать тело как есть.
    Write,
    /// Не писать: в ответе секрет ключа (GET /v4/user/account/api-key на api.xt.com — поле secretKey). Это адрес
    /// user-center, не спот: пауза после 429 — своя (slot(false)).
    Secret,
}

/// Что делает: разбор ответа спота. Принимает: окружение, путь, ответ транспорта, операцию, как писать в журнал.
/// Возвращает: поле result. Ошибки: RateLimited (429, GATEWAY_*), коды XT (errors.rs), BadResponse — не JSON / нет rc.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op, raw: Raw) -> GatewayResult<Value> {
    if raw == Raw::Write {
        ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    }
    let text = if raw == Raw::Write {
        r.text.as_str()
    } else {
        ""
    };
    let status = r.status.as_u16();
    if status == 429 {
        return Err(errors::from_http(
            status,
            text,
            &on_limit(ctx, raw == Raw::Write),
        ));
    }
    let Ok(mut v) = serde_json::from_str::<Value>(&r.text) else {
        return Err(errors::from_http(status, text, ""));
    };
    match v.get("rc").and_then(Value::as_i64) {
        Some(0) if status < 400 => Ok(v.get_mut("result").map(Value::take).unwrap_or(Value::Null)),
        Some(_) => {
            let mc = v.get("mc").and_then(Value::as_str).unwrap_or_default();
            if errors::is_risk_control(mc) {
                on_limit(ctx, raw == Raw::Write);
            }
            let ma = v.get("ma").map(Value::to_string).unwrap_or_default();
            Err(errors::from_code(mc, &ma, op, text))
        }
        None if status >= 400 => Err(errors::from_http(status, text, "")),
        None => Err(errors::bad("в ответе нет rc", text)),
    }
}

/// Что делает: разбор ответа фьючерсов (панель «Балансы», только чтение). Принимает: окружение, путь, ответ.
/// Возвращает: поле result. Ошибки: RateLimited (429), BadResponse — returnCode не 0/200, не JSON.
pub fn parse_futures(ctx: &Ctx, path: &str, r: &Reply) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    if status == 429 {
        return Err(errors::from_http(status, &r.text, &on_limit(ctx, false)));
    }
    let Ok(mut v) = serde_json::from_str::<Value>(&r.text) else {
        return Err(errors::from_http(status, &r.text, ""));
    };
    match v.get("returnCode").and_then(Value::as_i64) {
        Some(0 | 200) if status < 400 => {
            Ok(v.get_mut("result").map(Value::take).unwrap_or(Value::Null))
        }
        _ => {
            let e = v.get("error").map(Value::to_string).unwrap_or_default();
            let info = v.get("msgInfo").map(Value::to_string).unwrap_or_default();
            Err(errors::bad(
                &format!("фьючерсы XT, HTTP {status}: {info} {e}"),
                &r.text,
            ))
        }
    }
}
