//! Файл: venues/kucoin/reply.rs
//! Что делает: разбор ответа REST KuCoin (код HTTP, тело {code, data, msg}) в значение или единую ошибку шлюза и пауза
//! запросов ресурсного пула после 429000 (не слать дальше — урок 15). Ответы пишутся в сырой журнал, кроме путей
//! токенов WebSocket (bullet: в ответе — токен подключения).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: нет.
//!
//! Официально (https://www.kucoin.com/docs-new/rate-limit-rule-classic): у REST ресурсные пулы — Spot, Management,
//! Public и др.; квоты — «per 30 seconds»; превышение — HTTP 429 и код 429000, «stop sending requests and wait for the
//! pool to reset», время до сброса — заголовок gw-ratelimit-reset (мс). Заголовков ответа, кроме Retry-After, общий
//! HTTP-обмен шлюза (gateway/http.rs) наружу не отдаёт (запрос на изменение стыка — docs/TASKS.md), поэтому пул
//! ставится на паузу на всё окно квоты — 30 с: за это время пул сбрасывается гарантированно. Ответ успеха —
//! {"code":"200000","data":…} (примеры всех эндпоинтов спецификации SDK).

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// Код успеха KuCoin — из официальных примеров ответов («code": "200000"»).
pub const OK_CODE: &str = "200000";
/// Окно квоты ресурсного пула, мс — официально «per 30 seconds» (rate-limit-rule-classic).
const POOL_WINDOW_MS: i64 = 30_000;

/// Ресурсный пул запроса (поле x-api-rate-limit-pool спецификации каждого эндпоинта).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// Публичные (по IP).
    Public,
    /// Spot (по UID): ордера, комиссии, токен приватного WebSocket.
    Spot,
    /// Management (по UID): счета, переводы, ввод и вывод, ключ.
    Management,
    /// Futures (по UID): фьючерсный домен — только чтение балансов панели (all_balances.rs).
    Futures,
    /// Earn (по UID): вклады Earn — только чтение балансов панели (all_balances.rs).
    Earn,
}

/// До какого момента (unix мс) пул закрыт. Один адаптер KuCoin на процесс — состояние модуля. 0 — открыт.
static PAUSE: [AtomicI64; 5] = [
    AtomicI64::new(0),
    AtomicI64::new(0),
    AtomicI64::new(0),
    AtomicI64::new(0),
    AtomicI64::new(0),
];

/// Номер пула в PAUSE. Ошибок нет.
fn slot(p: Pool) -> usize {
    match p {
        Pool::Public => 0,
        Pool::Spot => 1,
        Pool::Management => 2,
        Pool::Futures => 3,
        Pool::Earn => 4,
    }
}

/// Что делает: не пускает запрос, пока пул на паузе после 429000.
/// Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate(p: Pool) -> GatewayResult<()> {
    let left = PAUSE[slot(p)].load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!(
                "KuCoin: пул {p:?} на паузе после 429000 — ещё {left} мс запросы не отправляются"
            ),
        ));
    }
    Ok(())
}

/// Ставит паузу пула на окно квоты от текущего момента. Ошибок нет.
fn pause(p: Pool) {
    let until = chrono::Utc::now().timestamp_millis() + POOL_WINDOW_MS;
    PAUSE[slot(p)].fetch_max(until, Ordering::Relaxed);
}

/// Что делает: разбор ответа.
/// Принимает: окружение, путь (для журнала), ответ, операцию, пул.
/// Возвращает: поле data ответа (забирается без копии).
/// Ошибки: RateLimited (HTTP 429, код 429000 — пул на паузу), коды KuCoin (errors.rs), BadResponse — не JSON.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op, pool: Pool) -> GatewayResult<Value> {
    if !path.contains("/bullet-") {
        ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    }
    let status = r.status.as_u16();
    let Ok(mut v) = serde_json::from_str::<Value>(&r.text) else {
        if status == 429 {
            pause(pool);
        }
        return Err(errors::from_http(status, &r.text));
    };
    let code = v.get("code").and_then(code_text);
    match code.as_deref() {
        Some(OK_CODE) if status < 400 => Ok(v
            .as_object_mut()
            .and_then(|m| m.remove("data"))
            .unwrap_or(Value::Null)),
        Some(c) => {
            if c == "429000" || status == 429 {
                pause(pool);
            }
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or_default();
            Err(errors::from_code(c, msg, op, &r.text))
        }
        None => {
            if status == 429 {
                pause(pool);
            }
            Err(errors::from_http(status, &r.text))
        }
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
