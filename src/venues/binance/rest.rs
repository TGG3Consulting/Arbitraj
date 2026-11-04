//! Файл: venues/binance/rest.rs
//! Что делает: запросы REST к Binance — публичные и подписанные (HMAC SHA256, sign.rs), разбор ответа (reply.rs),
//! пауза после 418/429, переход на резервный адрес только при сбое сети. Большой ответ разбирается вне рабочих
//! потоков tokio (heavy: exchangeInfo, config/getall — решение 131).
//! Кто вызывает: файлы адаптера Binance.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, venue.recv_window_ms [binance], http.*; секреты
//! binance.api_key, binance.api_secret.
//!
//! Официально (https://developers.binance.com/legacy-docs/binance-spot-api-docs/rest-api/request-security): ключ —
//! заголовок X-MBX-APIKEY; SIGNED — параметры timestamp (мс) и signature; recvWindow «Maximum 60000»; параметры
//! «may be sent in the query string» — все параметры (в том числе у POST/DELETE) шлются строкой запроса, тела нет
//! (payload подписи = строка запроса). Подпись SAPI (Wallet) — та же.

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{reply, sign};

/// Предел recvWindow из официальной документации («Maximum: 60000»).
const MAX_RECV_WINDOW_MS: u64 = 60_000;

/// Параметры запроса: пары имя — значение (порядок сохраняется — он же порядок подписи).
pub type Params = Vec<(&'static str, String)>;

tokio::task_local! {
    /// Разбор ответа — вне рабочих потоков (public_heavy, signed_heavy).
    static HEAVY: bool;
}

/// Публичный GET (без подписи). Ошибки: связь, коды Binance, пауза после 429.
pub async fn public(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    let q = sign::query(&params).map_err(|m| internal(ctx, m))?;
    send(ctx, Method::GET, path, &q, None, Op::Other).await
}

/// Публичный GET с большим ответом (exchangeInfo): разбор JSON — в spawn_blocking. Ошибки: как public.
pub async fn public_heavy(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    HEAVY.scope(true, public(ctx, path, params)).await
}

/// Подписанный запрос. Принимает: метод, путь, параметры (без timestamp/recvWindow/signature), вид операции.
/// Возвращает: тело ответа. Ошибки: SecretsLocked/NotConfigured (ключи, recvWindow), коды Binance, связь.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    mut params: Params,
    op: Op,
) -> GatewayResult<Value> {
    let (key, secret) = ctx.keys()?;
    params.push(("recvWindow", recv_window(ctx)?.to_string()));
    params.push((
        "timestamp",
        chrono::Utc::now().timestamp_millis().to_string(),
    ));
    let canon = sign::query(&params).map_err(|m| internal(ctx, m))?;
    let signature = sign::hmac_hex(&secret, &canon)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::InvalidCredentials, None, m)))?;
    let full = format!("{canon}&signature={signature}");
    send(ctx, method, path, &full, Some(key.as_str()), op).await
}

/// Подписанный запрос с большим ответом (config/getall): разбор — в spawn_blocking. Ошибки: как signed.
pub async fn signed_heavy(
    ctx: &Ctx,
    method: Method,
    path: &str,
    params: Params,
    op: Op,
) -> GatewayResult<Value> {
    HEAVY
        .scope(true, signed(ctx, method, path, params, op))
        .await
}

/// venue.recv_window_ms с проверкой предела документации. Ошибка NotConfigured — не задана или больше 60000.
pub fn recv_window(ctx: &Ctx) -> GatewayResult<u64> {
    let recv = ctx.int("venue.recv_window_ms")?;
    if recv > MAX_RECV_WINDOW_MS {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.recv_window_ms",
            format!("venue.recv_window_ms = {recv}: у Binance не больше {MAX_RECV_WINDOW_MS}"),
        )));
    }
    Ok(recv)
}

/// Отправка с переходом на резервный адрес при сбое сети. key — заголовок X-MBX-APIKEY (None — публичный запрос).
async fn send(
    ctx: &Ctx,
    m: Method,
    path: &str,
    query: &str,
    key: Option<&str>,
    op: Op,
) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let lim = ctx.limits()?;
    // Пауза после 418/429: запрос не отправляется вовсе (продолжение после 429 — бан IP).
    reply::gate(path)?;
    let mut last = None;
    for base in &urls {
        let url = if query.is_empty() {
            format!("{base}{path}")
        } else {
            format!("{base}{path}?{query}")
        };
        let mut req = ctx.http.request(m.clone(), url);
        if let Some(key) = key {
            req = req.header("X-MBX-APIKEY", key);
        }
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(r) => {
                let parsed = if HEAVY.try_with(|h| *h).unwrap_or(false) {
                    let (c, p) = (ctx.clone(), path.to_string());
                    tokio::task::spawn_blocking(move || reply::parse(&c, &p, &r, op))
                        .await
                        .map_err(|e| internal(ctx, format!("разбор ответа {path} прерван: {e}")))?
                } else {
                    reply::parse(ctx, path, &r, op)
                };
                return parsed.map_err(|e| errors::unsure(e, op));
            }
            // Резерв — только если запрос точно не дошёл; иначе ордер или вывод мог исполниться — не повторяем.
            Err(f) if f.not_sent() || (op == Op::Other && f.is_timeout()) => {
                last = Some(errors::from_fail(&f, &lim))
            }
            Err(f) => return Err(errors::fail_for(&f, &lim, op)),
        }
    }
    Err(last.unwrap_or_else(|| {
        GatewayError::new(
            ErrorCode::NotConfigured,
            Some(ctx.venue()),
            "Не задан адрес REST",
        )
    }))
}

/// Внутренняя ошибка с площадкой Binance. Ошибок нет.
fn internal(ctx: &Ctx, m: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::Internal, None, m))
}
