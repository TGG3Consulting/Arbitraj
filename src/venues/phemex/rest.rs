//! Файл: venues/phemex/rest.rs
//! Что делает: запросы REST к Phemex — публичные и подписанные (sign.rs), разбор ответа (reply.rs), пауза после 429,
//! переход на резервный адрес только при сбое сети. Большой ответ (справочник пар и монет) разбирается вне рабочих
//! потоков tokio (решение 131). Сжатие gzip — у общего HTTP-клиента (решение 142).
//! Кто вызывает: файлы адаптера Phemex.
//! От чего зависит: ctx.rs, sign.rs, reply.rs, errors.rs, gateway/http.rs, reqwest.
//! Настройки: venue.rest_url, venue.rest_fallback_url, venue.recv_window_ms [phemex] (срок x-phemex-request-expiry от
//! «сейчас»), http.*; секреты phemex.*.
//!
//! Официально (General API information, https://phemex-docs.github.io/#rest-request-header): подписывается «URL Path +
//! QueryString + Expiry + body»; expiry — секунды, «normally it should be (Now() + 1 minute)» — срок задаёт Тигран
//! (venue.recv_window_ms, округление вверх до секунды). Порядок и кодирование параметров строки в документации не
//! описаны: строка подписывается «как отправлена» (пример 2 — повторяющийся ordStatus); официальный пример клиента
//! (phemex-python-api, client.py: '&'.join('{}={}'…)) шлёт параметры в заданном порядке без кодирования — так и здесь:
//! значения только из безопасных знаков (буквы, цифры, «_-.:»), тогда строка в адресе и в подписи одна и та же.

use reqwest::Method;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{reply, sign};

/// Параметры строки запроса: пары ключ — значение (отправляются в заданном порядке).
pub type Params = Vec<(&'static str, String)>;

/// Один запрос.
pub struct Req<'a> {
    pub method: Method,
    /// Путь без строки запроса (например /spot/orders/create).
    pub path: &'a str,
    pub query: Params,
    /// Тело JSON (POST /wallets/account/transfer).
    pub body: Option<Value>,
    pub op: Op,
    /// Подписанный запрос.
    pub signed: bool,
    /// Разбор ответа вне рабочих потоков (большой ответ).
    pub heavy: bool,
}

/// Публичный GET. Принимает: путь, параметры. Возвращает: data ответа. Ошибки: связь, коды Phemex.
pub async fn public(ctx: &Ctx, path: &str, query: Params, heavy: bool) -> GatewayResult<Value> {
    let r = Req {
        method: Method::GET,
        path,
        query,
        body: None,
        op: Op::Other,
        signed: false,
        heavy,
    };
    send(ctx, r).await
}

/// Подписанный запрос (кратко). Принимает: метод, путь, параметры строки, тело, операцию.
/// Возвращает: data ответа. Ошибки: SecretsLocked/NotConfigured (ключи, срок), коды Phemex.
pub async fn signed(
    ctx: &Ctx,
    method: Method,
    path: &str,
    query: Params,
    body: Option<Value>,
    op: Op,
) -> GatewayResult<Value> {
    let r = Req {
        method,
        path,
        query,
        body,
        op,
        signed: true,
        heavy: false,
    };
    send(ctx, r).await
}

/// Что делает: отправка запроса на адреса venue.rest_url (+ резерв только при сбое сети).
/// Принимает: окружение, запрос. Возвращает: data ответа. Ошибки: как public / signed.
pub async fn send(ctx: &Ctx, r: Req<'_>) -> GatewayResult<Value> {
    let urls = ctx.rest_urls()?;
    let q = query(&r.query).map_err(|e| ctx.tag(e))?;
    let path_q = if q.is_empty() {
        r.path.to_string()
    } else {
        format!("{}?{q}", r.path)
    };
    let body = r.body.as_ref().map(Value::to_string).unwrap_or_default();
    let lim = ctx.limits()?;
    let window_s = if r.signed {
        ctx.int("venue.recv_window_ms")?.div_ceil(1000) as i64
    } else {
        0
    };
    // Пауза после 429 — запрос не отправляется вовсе.
    reply::gate().map_err(|e| ctx.tag(e))?;
    let mut last = None;
    for base in &urls {
        let mut req = ctx
            .client()?
            .request(r.method.clone(), format!("{base}{path_q}"));
        if r.signed {
            let keys = ctx.keys()?;
            let expiry = chrono::Utc::now().timestamp() + window_s;
            let h = sign::headers(&keys, r.path, &q, expiry, &body).map_err(|e| ctx.tag(e))?;
            req = req
                .header("x-phemex-access-token", h.token)
                .header("x-phemex-request-expiry", h.expiry)
                .header("x-phemex-request-signature", h.signature);
        }
        if !body.is_empty() {
            req = req
                .header("Content-Type", "application/json")
                .body(body.clone());
        }
        match crate::gateway::http::exchange_full(req, lim).await {
            Ok(reply) => {
                let parsed = if r.heavy {
                    let (c, p, op) = (ctx.clone(), r.path.to_string(), r.op);
                    tokio::task::spawn_blocking(move || reply::parse(&c, &p, &reply, op))
                        .await
                        .map_err(|e| {
                            ctx.tag(GatewayError::new(
                                ErrorCode::Internal,
                                None,
                                format!("разбор ответа {} прерван: {e}", r.path),
                            ))
                        })?
                } else {
                    reply::parse(ctx, r.path, &reply, r.op)
                };
                return parsed.map_err(|e| errors::unsure(e, r.op));
            }
            // Резерв — только если запрос точно не дошёл; иначе ордер или вывод мог исполниться — не повторяем.
            Err(f) if f.not_sent() || (r.op == Op::Other && f.is_timeout()) => {
                last = Some(errors::from_fail(&f, &lim))
            }
            Err(f) => return Err(errors::fail_for(&f, &lim, r.op)),
        }
    }
    Err(last.unwrap_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "Не задан адрес REST Phemex",
        ))
    }))
}

/// Строка запроса: «имя=значение» через «&» в заданном порядке, без кодирования (см. шапку).
/// Ошибка Internal — в значении знак, который потребовал бы кодирования (строка в адресе и в подписи разошлась бы).
pub fn query(params: &Params) -> GatewayResult<String> {
    let mut out = Vec::with_capacity(params.len());
    for (k, v) in params {
        let safe = v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'));
        if !safe || v.is_empty() {
            return Err(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("Недопустимое значение параметра {k} запроса Phemex: «{v}»"),
            ));
        }
        out.push(format!("{k}={v}"));
    }
    Ok(out.join("&"))
}
