//! Файл: venues/bybit/reply.rs
//! Что делает: разбор ответа REST Bybit V5 (HTTP-код, обёртка {retCode, retMsg, result}) в значение или единую ошибку
//! шлюза и паузы запросов по лимитам (урок 15): после HTTP 403 — ни одного запроса к Bybit 10 минут; по заголовкам
//! X-Bapi-Limit-Status = 0 и при retCode 10006 — ни одного запроса к этому эндпоинту до X-Bapi-Limit-Reset-Timestamp.
//! Ответы пишутся в сырой журнал (raw.*).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, http.rs (Reply).
//! Настройки: нет.
//!
//! Официальная документация:
//! - «Rate Limit Rules» https://bybit-exchange.github.io/docs/v5/rate-limit — «If you encounter the error "403, access
//!   too frequent" … terminate all HTTP sessions and wait for at least 10 minutes»; «retCode 10006, Too many visits!» —
//!   лимит «per second per UID» по эндпоинту; заголовки X-Bapi-Limit*.
//! - «Integration Guidance → Common response parameters» https://bybit-exchange.github.io/docs/v5/guide — retCode
//!   «Success/Error code», retMsg «OK, success, SUCCESS, "" indicates a successful response», result — данные.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, RwLock};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};
use super::http::Reply;

/// Пауза после HTTP 403 — из официальной документации («wait for at least 10 minutes»).
const IP_BAN_PAUSE_MS: i64 = 10 * 60 * 1000;

/// До какого момента (Unix мс) не слать ни одного запроса (после 403). Один адаптер Bybit на процесс. 0 — паузы нет.
static IP_PAUSE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// Паузы по эндпоинтам: путь → до какого момента (Unix мс) не слать.
static PATH_PAUSE: LazyLock<RwLock<HashMap<String, i64>>> = LazyLock::new(Default::default);

/// Сейчас, Unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Что делает: остаток паузы, мс (≤ 0 — паузы нет) — для терпения ручной работы (решение 201, gateway/patience.rs).
/// Принимает: то же, что gate. Ошибок нет.
pub fn pause_left_ms(path: &str) -> i64 {
    let now = now_ms();
    let ip = IP_PAUSE_UNTIL_MS.load(Ordering::Relaxed);
    let p = PATH_PAUSE
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(path)
        .copied()
        .unwrap_or(0);
    ip.max(p) - now
}

/// Что делает: не пускает запрос, пока идёт пауза (вся площадка после 403 или этот эндпоинт по лимиту UID).
/// Принимает: путь. Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate(path: &str) -> GatewayResult<()> {
    let now = now_ms();
    let ip = IP_PAUSE_UNTIL_MS.load(Ordering::Relaxed) - now;
    if ip > 0 {
        return Err(limited(format!(
            "Bybit: пауза после HTTP 403 (лимит IP) — ещё {ip} мс запросы не отправляются"
        )));
    }
    let until = PATH_PAUSE
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(path)
        .copied()
        .unwrap_or(0);
    if until > now {
        return Err(limited(format!(
            "Bybit {path}: лимит запросов исчерпан — ещё {} мс запросы не отправляются (X-Bapi-Limit-Reset-Timestamp)",
            until - now
        )));
    }
    Ok(())
}

/// Что делает: пауза эндпоинта до момента сброса лимита (дальняя из старой и новой). Ошибок нет.
fn pause_path(path: &str, until: i64) {
    if until <= now_ms() {
        return;
    }
    let mut m = PATH_PAUSE.write().unwrap_or_else(|p| p.into_inner());
    let e = m.entry(path.to_string()).or_insert(0);
    *e = (*e).max(until);
}

/// Что делает: разбор ответа. Принимает: окружение, путь, ответ, операцию.
/// Возвращает: поле result. Ошибки: RateLimited (403, 429, 10006), коды Bybit (errors.rs), BadResponse.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    // Последний разрешённый запрос этого эндпоинта в окне: следующие — только после сброса.
    if r.left == Some(0)
        && let Some(reset) = r.reset_ms
    {
        pause_path(path, reset);
    }
    if status == 403 {
        IP_PAUSE_UNTIL_MS.fetch_max(now_ms() + IP_BAN_PAUSE_MS, Ordering::Relaxed);
        return Err(errors::from_http(status, &r.text));
    }
    let Ok(mut v) = serde_json::from_str::<Value>(&r.text) else {
        return Err(errors::from_http(status, &r.text));
    };
    let Some(code) = v.get("retCode").and_then(Value::as_i64) else {
        return Err(errors::from_http(status, &r.text));
    };
    if code == 10006
        && let Some(reset) = r.reset_ms
    {
        pause_path(path, reset);
    }
    if code != 0 {
        let msg = v.get("retMsg").and_then(Value::as_str).unwrap_or_default();
        let mut e = errors::from_code(code, msg, op, &r.text);
        // Дополнение 268 (ревью 2): при HTTP 5XX код PriceOutOfRange не присваивается (как до решения 268 — OrderRejected):
        // по ответу с ошибкой сервера рыночный ордер не ставится.
        if status >= 500 {
            e = e.no_price_range();
        }
        if code == 10006 {
            e.message = format!(
                "{} — запросы к {path} не отправляются до сброса лимита (лимит {} в секунду)",
                e.message,
                r.limit.map_or("не сообщён".into(), |l| l.to_string())
            );
        }
        return Err(e);
    }
    if status >= 400 {
        return Err(errors::from_http(status, &r.text));
    }
    // Решение 131: result забирается из ответа, а не копируется (у coin/query-info ответ большой).
    Ok(v.as_object_mut()
        .and_then(|m| m.remove("result"))
        .unwrap_or(Value::Null))
}

/// Ошибка RateLimited с площадкой Bybit.
fn limited(msg: String) -> GatewayError {
    GatewayError::new(
        ErrorCode::RateLimited,
        Some(crate::core::types::VenueId(ID.to_string())),
        msg,
    )
}
