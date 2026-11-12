//! Файл: venues/xt/rest.rs
//! Что делает: запросы REST к XT.com — публичные (/v4/public/…) и подписанные (заголовки validate-*, sign.rs), разбор
//! ответа (reply.rs), пауза после 429. Тело POST — JSON одной строкой: та же строка подписывается и уходит в запрос.
//! Большой ответ разбирается вне рабочих потоков tokio (heavy: справочники пар и сетей — решение 131). Отдельно —
//! user-center (ответ с секретом ключа не пишется в журнал); фьючерсы — futures.rs.
//! Кто вызывает: файлы адаптера XT.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.recv_window_ms [xt], http.*; секреты xt.api_key, xt.api_secret.
//!
//! Официально (https://doc.xt.com/docs/spot/Access%20Description/BasicInformationOfTheInterface, `_documentation/apiBasicInfo.md`): «GET request parameters
//! are placed in query Params, POST request parameters are placed in request body», «Content-Type=application/json»,
//! «For requests that start other than /public, the request message needs to be signed». recvWindow: текст
//! противоречив («between 2 to 60 seconds», «older than 5,000 milliseconds is invalid», «longer than 5 seconds is not
//! recommended», пример — 60000): предел 60000 мс, значение — решение Тиграна (venue.recv_window_ms).

use std::sync::atomic::{AtomicU8, Ordering};

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::reply::{self, Raw};
use super::sign::{self, SignForm};

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
    // Решение 207: значение, которое нельзя поставить в адрес как есть (пробел в «BNB Smart Chain»), — только в GET
    // (адрес депозита, история); вариант подписи — запомненный, иначе сначала закодированная строка, при отказе подписи
    // (AUTH_103) — исходная. Простые значения — как раньше (одна строка в адрес и в подпись).
    let (_, _, special) = sign::query_forms(&params, SignForm::Encoded);
    if !special {
        let query = sign::query(&params).map_err(|m| internal(ctx, m))?;
        return signed_once(
            ctx,
            &base,
            &method,
            path,
            (query.clone(), query),
            &body,
            op,
            raw,
        )
        .await;
    }
    if method != Method::GET {
        sign::query(&params).map_err(|m| internal(ctx, m))?;
    }
    let known = form_known();
    let order = match known {
        Some(f) => vec![f],
        None => vec![SignForm::Encoded, SignForm::Raw],
    };
    let mut last = None;
    for form in order {
        let (url, sig, _) = sign::query_forms(&params, form);
        match signed_once(ctx, &base, &method, path, (url, sig), &body, op, raw).await {
            Ok(v) => {
                if known.is_none() {
                    remember(form);
                    let m = format!(
                        "XT принял подпись строки запроса с пробелом: вариант {form:?} (решение 207)"
                    );
                    ctx.log(Level::Info, "venue.sign", m).await;
                }
                return Ok(v);
            }
            Err(e) if known.is_none() && e.message.contains("AUTH_103") => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| internal(ctx, "подпись XT не принята".into())))
}

/// Запомненный вариант подписи (0 — не выяснен, 1 — Encoded, 2 — Raw). На время процесса.
static FORM: AtomicU8 = AtomicU8::new(0);

fn form_known() -> Option<SignForm> {
    match FORM.load(Ordering::Relaxed) {
        1 => Some(SignForm::Encoded),
        2 => Some(SignForm::Raw),
        _ => None,
    }
}

fn remember(f: SignForm) {
    FORM.store(
        if f == SignForm::Encoded { 1 } else { 2 },
        Ordering::Relaxed,
    );
}

/// Один подписанный запрос. Принимает: (строка в адрес, строка в подпись). Ошибки: как у send.
#[allow(clippy::too_many_arguments)]
async fn signed_once(
    ctx: &Ctx,
    base: &str,
    method: &Method,
    path: &str,
    (query, signed_query): (String, String),
    body: &Option<Value>,
    op: Op,
    raw: Raw,
) -> GatewayResult<Value> {
    let (key, secret) = ctx.keys()?;
    let recv = recv_window(ctx)?;
    let ts = chrono::Utc::now().timestamp_millis();
    let body = body.as_ref().map(|b| b.to_string()).unwrap_or_default();
    let original = format!(
        "{}{}",
        sign::spot_headers(&key, recv, ts),
        sign::data(method.as_str(), path, &signed_query, &body)
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
        base: base.to_string(),
        method: method.clone(),
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
