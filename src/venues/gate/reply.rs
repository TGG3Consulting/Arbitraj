//! Файл: venues/gate/reply.rs
//! Что делает: разбор ответа REST Gate (код HTTP, тело) в значение или единую ошибку шлюза и пауза запросов после
//! ответа «слишком часто» (HTTP 429 / TOO_MANY_REQUESTS) — до её конца адаптер сам не шлёт ни одного запроса.
//! Ответы пишутся в сырой журнал (raw.*).
//! Кто вызывает: rest.rs (каждый ответ; gate — перед каждым запросом).
//! От чего зависит: errors.rs, ctx.rs, gateway/http.rs (Reply).
//! Настройки: venue.rate_limit_pause_ms [gate] — пауза после 429, если Gate не прислал Retry-After.
//!
//! Официальная документация, API v4 → «Frequency limit rule» https://www.gate.com/docs/developers/apiv4/en/ : лимиты
//! по конечной точке (спот: постановка и изменение ордера — 10 запросов/с на UID и рынок, отмена — 200/с, прочие
//! приватные — 200 за 10 с, публичные — 200 за 10 с на IP), заголовки X-Gate-RateLimit-Requests-Remain,
//! X-Gate-RateLimit-Limit, X-Gate-RateLimit-Reset-Timestamp; превышение — статус 429 «Too many requests». Заголовка
//! Retry-After в документации нет; X-Gate-RateLimit-Reset-Timestamp транспорт шлюза (gateway/http.rs) не отдаёт — менять
//! шлюз адаптеру нельзя, поэтому пауза: Retry-After, если он всё же пришёл, иначе venue.rate_limit_pause_ms (вопрос
//! Тиграну в docs/TASKS.md). Успех — тело JSON (объект или массив) со статусом 2xx; ошибка — {label, message}.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::http::Reply;

use super::ctx::{Ctx, ID};
use super::errors::{self, Op};

/// До какого момента (unix мс) запросы к Gate не отправляются. Один адаптер Gate на процесс. 0 — паузы нет.
static PAUSE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// Что делает: не пускает запрос, пока идёт пауза после 429.
/// Возвращает: Ok — можно слать. Ошибки: RateLimited — сколько ещё ждать.
pub fn gate() -> GatewayResult<()> {
    let left = PAUSE_UNTIL_MS.load(Ordering::Relaxed) - chrono::Utc::now().timestamp_millis();
    if left > 0 {
        return Err(GatewayError::new(
            ErrorCode::RateLimited,
            Some(crate::core::types::VenueId(ID.to_string())),
            format!("Gate: пауза после HTTP 429 — ещё {left} мс запросы не отправляются"),
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
/// Возвращает: тело JSON. Ошибки: RateLimited (429), метки Gate (errors.rs), BadResponse — тело не JSON.
pub fn parse(ctx: &Ctx, path: &str, r: &Reply, op: Op) -> GatewayResult<Value> {
    ctx.raw.write(ID, &format!("rest {path}"), &r.text);
    let status = r.status.as_u16();
    let v = serde_json::from_str::<Value>(&r.text).ok();
    if (200..300).contains(&status) {
        return v.ok_or_else(|| errors::bad(&format!("{path}: тело не JSON"), &r.text));
    }
    let label = v
        .as_ref()
        .and_then(|v| v.get("label"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let limited = status == 429 || label.as_deref() == Some("TOO_MANY_REQUESTS");
    let note = if limited {
        Some(on_limit(ctx, r.retry_after_s))
    } else {
        None
    };
    let mut e = match label {
        Some(l) => {
            let v = v.as_ref();
            let msg = v
                .and_then(|v| v.get("message").or_else(|| v.get("detail")))
                .and_then(Value::as_str)
                .unwrap_or_default();
            errors::from_label(status, &l, msg, op, &r.text)
        }
        None => errors::from_http(status, &r.text, r.retry_after_s),
    };
    if let Some(n) = note {
        e.message = format!("{} — {n}", e.message);
    }
    Err(e)
}
