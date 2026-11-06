//! Файл: venues/xt/rest.rs
//! Что делает: запросы REST к XT.com — публичные (/v4/public/…) и подписанные (заголовки validate-*, sign.rs), разбор
//! ответа (reply.rs), пауза после 429. Тело POST — JSON одной строкой: та же строка подписывается и уходит в запрос.
//! Большой ответ разбирается вне рабочих потоков tokio (heavy: справочники пар и сетей — решение 131). Отдельно —
//! user-center (ответ с секретом ключа не пишется в журнал); фьючерсы — futures.rs.
//! Кто вызывает: файлы адаптера XT.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.recv_window_ms [xt], http.*; секреты xt.api_key, xt.api_secret.
//!
//! Официально (https://doc.xt.com/#documentationapiBasicInfo, `_documentation/apiBasicInfo.md`): «GET request parameters
//! are placed in query Params, POST request parameters are placed in request body», «Content-Type=application/json»,
//! «For requests that start other than /public, the request message needs to be signed». recvWindow: текст
//! противоречив («between 2 to 60 seconds», «older than 5,000 milliseconds is invalid», «longer than 5 seconds is not
//! recommended», пример — 60000): предел 60000 мс, значение — решение Тиграна (venue.recv_window_ms).

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::reply::{self, Raw};
use super::sign;

/// Предел recvWindow — верхняя граница из документации («between 2 to 60 seconds»).
const MAX_RECV_WINDOW_MS: u64 = 60_000;

/// Параметры строки запроса: пары имя — значение (порядок не важен — подпись сортирует, sign.rs).
pub type Params = Vec<(&'static str, String)>;

tokio::task_local! {
    /// Разбор ответа — вне рабочих потоков (public_heavy).
    static HEAVY: bool;
}

/// Запрос целиком: куда, что и как подписан.
struct Call<'a> {
    base: String,
    method: Method,
    path: &'a str,
    query: String,
    body: String,
    /// Заголовки validate-* (пусто — публичный запрос).
    headers: Vec<(&'static str, String)>,
    op: Op,
    raw: Raw,
}

/// Публичный GET (без подписи). Ошибки: связь, коды XT, пауза после 429.
pub async fn public(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    let query = sign::query(&params).map_err(|m| internal(ctx, m))?;
    let call = Call {
        base: ctx.rest_url()?,
        method: Method::GET,
        path,
        query,
        body: String::new(),
        headers: vec![],
        op: Op::Other,
        raw: Raw::Write,
    };
    send(ctx, call).await
}

/// Публичный GET с большим ответом: разбор — в spawn_blocking. Ошибки: как public.
pub async fn public_heavy(ctx: &Ctx, path: &str, params: Params) -> GatewayResult<Value> {
    HEAVY.scope(true, public(ctx, path, params)).await
}

/// Подписанный запрос к споту. Принимает: метод, путь, строку запроса, тело JSON (None — без тела), вид операции.
/// Возвращает: result. Ошибки: SecretsLocked/NotConfigured (ключи, recvWindow), коды XT, связь.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    params: Params,
    body: Option<Value>,
    op: Op,
) -> GatewayResult<Value> {
    let base = ctx.rest_url()?;
    signed_at(ctx, base, method, path, params, body, op, Raw::Write).await
}

/// Подписанный GET с ответом, где есть секрет ключа (user-center), по адресу base. Ответ в журнал не пишется.
/// Ошибки: как signed.
pub async fn signed_secret(
    ctx: &Ctx,
    base: String,
    path: &str,
    params: Params,
) -> GatewayResult<Value> {
    let base = base.trim_end_matches('/').to_string();
    signed_at(
        ctx,
        base,
        Method::GET,
        path,
        params,
        None,
        Op::Other,
        Raw::Secret,
    )
    .await
}

/// Подписанный запрос по адресу base. Ошибки: как signed.
#[allow(clippy::too_many_arguments)]
async fn signed_at(
    ctx: &Ctx,
    base: String,
    method: Method,
    path: &str,
    params: Params,
    body: Option<Value>,
    op: Op,
    raw: Raw,
) -> GatewayResult<Value> {
    let (key, secret) = ctx.keys()?;
    let recv = recv_window(ctx)?;
    let ts = chrono::Utc::now().timestamp_millis();
    let query = sign::query(&params).map_err(|m| internal(ctx, m))?;
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let original = format!(
        "{}{}",
        sign::spot_headers(&key, recv, ts),
        sign::data(method.as_str(), path, &query, &body)
    );
    let signature = sign::hmac_hex(&secret, &original)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::InvalidCredentials, None, m)))?;
    let headers = vec![
        ("validate-algorithms", sign::ALGORITHM.to_string()),
        ("validate-appkey", key.to_string()),
        ("validate-recvwindow", recv.to_string()),
        ("validate-timestamp", ts.to_string()),
        ("validate-signature", signature),
    ];
    let call = Call {
        base,
        method,
        path,
        query,
        body,
        headers,
        op,
        raw,
    };
    send(ctx, call).await
}

/// venue.recv_window_ms с пределом документации. Ошибка NotConfigured — не задана или больше 60000.
pub fn recv_window(ctx: &Ctx) -> GatewayResult<u64> {
    let recv = ctx.int("venue.recv_window_ms")?;
    if recv > MAX_RECV_WINDOW_MS {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.recv_window_ms",
            format!("venue.recv_window_ms = {recv}: у XT не больше {MAX_RECV_WINDOW_MS} («between 2 to 60 seconds»)"),
        )));
    }
    Ok(recv)
}

/// Отправка одного запроса (без повторов: ордер или вывод мог исполниться). Ошибки: пауза, связь, ответ.
async fn send(ctx: &Ctx, c: Call<'_>) -> GatewayResult<Value> {
    let lim = ctx.limits()?;
    let spot = c.raw == Raw::Write;
    // Пауза после 429: запрос не отправляется вовсе (продолжение — блокировка IP или ключа).
    // Решение 201: ручная работа ждёт конец паузы (gateway/patience.rs).
    crate::gateway::patience::wait(reply::pause_left_ms(spot)).await;
    reply::gate(spot, c.path)?;
    let url = if c.query.is_empty() {
        format!("{}{}", c.base, c.path)
    } else {
        format!("{}{}?{}", c.base, c.path, c.query)
    };
    let mut req = ctx
        .client()?
        .request(c.method.clone(), url)
        .header("Content-Type", "application/json");
    for (k, v) in &c.headers {
        req = req.header(*k, v.as_str());
    }
    if !c.body.is_empty() {
        req = req.body(c.body.clone());
    }
    let (op, raw, path) = (c.op, c.raw, c.path.to_string());
    match crate::gateway::http::exchange_full(req, lim).await {
        Ok(r) => {
            let parsed = if HEAVY.try_with(|h| *h).unwrap_or(false) {
                let (cx, p) = (ctx.clone(), path.clone());
                tokio::task::spawn_blocking(move || reply::parse(&cx, &p, &r, op, raw))
                    .await
                    .map_err(|e| internal(ctx, format!("разбор ответа {path} прерван: {e}")))?
            } else {
                reply::parse(ctx, &path, &r, op, raw)
            };
            parsed.map_err(|e| errors::unsure(e, op))
        }
        Err(f) => Err(errors::fail_for(&f, &lim, op)),
    }
}

/// Внутренняя ошибка с площадкой XT. Ошибок нет.
fn internal(ctx: &Ctx, m: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::Internal, None, m))
}
