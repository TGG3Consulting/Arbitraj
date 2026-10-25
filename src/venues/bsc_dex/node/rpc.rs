//! Файл: venues/bsc_dex/node/rpc.rs
//! Что делает: JSON-RPC узла BSC по HTTP — одиночный запрос и пачка (batch): eth_call, eth_getLogs,
//! eth_getBalance, eth_estimateGas, eth_getTransactionReceipt и т.п. Плюс общий POST JSON для каналов отправки.
//! Кто вызывает: node/*, wallet/*, pools/*, check/*, exec/*, send/* (через post).
//! От чего зависит: ctx.rs (адрес bsc.node_http_url с ключом, http.timeout_ms), node/jsonrpc.rs, errors.rs.
//! Темп и повтор на «превышен лимит» — node/pace.rs (bsc.node_http_rps, правило шлюза RateLimited); сбой основного
//! узла — тот же запрос на резервный (node/failover.rs), и только потом правило шлюза.
//! Настройки: bsc.node_http_url, http.timeout_ms, bsc.node_http_rps; секрет bsc.node_http_key (если в адресе есть {key}).
//!
//! Транспорт — HTTP POST с телом JSON-RPC 2.0 (https://www.jsonrpc.org/specification, разделы 4–6);
//! пачка запросов — массив объектов, ответы сопоставляются по id (раздел 6 «Batch»).
//! Пачка уходит целиком одним запросом (техлид, Issue #7: batch оставить). Настройки «размер пачки» в каталоге
//! нет — запрос на изменение стыка в отчёте; до неё лимит провайдера на пачку виден ошибкой узла.
//! Методы и параметры — execution-apis: https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth
//! Адрес узла с ключом в журнал и ошибки не попадает (reqwest: without_url).

use alloy::rpc::json_rpc::Response;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::errors::{self, Op};
use super::failover::{self, Link, Node};
use super::{jsonrpc, pace};
use crate::gateway::http;

/// Источник ошибок для текста причины.
pub const SOURCE: &str = "узел BSC (HTTP)";
/// Имя секрета ключа провайдера для адреса HTTP.
pub const SECRET_HTTP_KEY: &str = "bsc.node_http_key";

/// Один запрос к узлу с приведением result к типу.
/// Принимает: метод, параметры (массив JSON). Возвращает: значение result.
/// Ошибки: NotConfigured/SecretsLocked (адрес, ключ), Timeout/Unreachable, ответ-ошибка узла, BadResponse.
pub async fn call<T: DeserializeOwned>(ctx: &Ctx, method: &str, params: Value) -> GatewayResult<T> {
    let raw = call_raw(ctx, method, params).await?;
    jsonrpc::decode(&raw, SOURCE)
}

/// Один запрос к узлу, result — сырой.
/// Принимает: метод, параметры. Возвращает: сырой result. Ошибки: как у call.
pub async fn call_raw(ctx: &Ctx, method: &str, params: Value) -> GatewayResult<Box<RawValue>> {
    let body = jsonrpc::request(1, method, &params);
    let text = pace::run(ctx, 1, || to_node(ctx, &body)).await?;
    jsonrpc::parse_text(&text, SOURCE, Op::Read)
}

/// Один запрос мимо общей очереди темпа (node/pace.rs) — только для опроса номера блока (node/heads.rs, решение 57):
/// его частоту задаёт bsc.blocks_poll_ms, и проверки не должны отодвигать его в очереди (от него зависит свежесть).
/// Принимает: метод, параметры. Возвращает: значение result. Ошибки: как у call (429 — сразу ошибкой, без повтора).
pub async fn call_unpaced<T: DeserializeOwned>(
    ctx: &Ctx,
    method: &str,
    params: Value,
) -> GatewayResult<T> {
    let body = jsonrpc::request(1, method, &params);
    let text = to_node(ctx, &body).await?;
    let raw = jsonrpc::parse_text(&text, SOURCE, Op::Read)?;
    jsonrpc::decode(&raw, SOURCE)
}

/// Пачка запросов одним HTTP-запросом (JSON-RPC batch).
/// Принимает: список (метод, параметры). Возвращает: результаты в том же порядке, у каждого свой Ok/Err.
/// Ошибки всей пачки: связь, HTTP, ответ не массив, нет ответа на какой-то id (BadResponse).
pub async fn batch(
    ctx: &Ctx,
    calls: &[(&str, Value)],
) -> GatewayResult<Vec<GatewayResult<Box<RawValue>>>> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let body = Value::Array(
        calls
            .iter()
            .enumerate()
            .map(|(i, (m, p))| jsonrpc::request(i as u64, m, p))
            .collect(),
    );
    let text = pace::run(ctx, calls.len(), || to_node(ctx, &body)).await?;
    let list: Vec<Response> = serde_json::from_str(&text).map_err(|e| {
        // Узел может ответить одним объектом-ошибкой на всю пачку.
        match jsonrpc::parse_text(&text, SOURCE, Op::Read) {
            Err(err) => err,
            Ok(_) => errors::bad(&format!("ответ на пачку не массив: {e}"), SOURCE, &text),
        }
    })?;
    let mut out: Vec<Option<GatewayResult<Box<RawValue>>>> =
        (0..calls.len()).map(|_| None).collect();
    for resp in list {
        let (id, res) = jsonrpc::parse(resp, SOURCE, Op::Read, &text);
        if let Some(slot) = id.and_then(|i| out.get_mut(i as usize)) {
            *slot = Some(res);
        }
    }
    out.into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.ok_or_else(|| errors::bad(&format!("нет ответа на запрос {i} пачки"), SOURCE, &text))
        })
        .collect()
}

/// Тело JSON-RPC узлу BSC: основной, при сбое (связь, таймаут, 5xx, 429) — резервный (node/failover.rs).
/// Принимает: тело. Возвращает: текст ответа. Ошибки: адрес/ключ не заданы; ошибка последнего узла.
async fn to_node(ctx: &Ctx, body: &Value) -> GatewayResult<String> {
    let urls = ctx.failover.urls(ctx, Link::Http)?;
    let n = urls.len();
    let mut last = None;
    for (i, (node, url)) in urls.into_iter().enumerate() {
        match post(ctx, &url, body, &[], SOURCE).await {
            Ok(t) => {
                if node == Node::Main {
                    ctx.failover.main_ok(ctx, Link::Http).await;
                }
                return Ok(t);
            }
            Err(e) if failover::switchable(&e) && i + 1 < n => {
                if node == Node::Main {
                    ctx.failover.trip(ctx, Link::Http, &e.message).await;
                }
                last = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| errors::bad("нет адреса узла", SOURCE, "")))
}

/// POST JSON по адресу и текст ответа; HTTP-статус не 2xx — ошибка по статусу.
/// Принимает: адрес (может содержать ключ — не логируется), тело, заголовки, источник для текста ошибки.
/// Возвращает: текст ответа. Ошибки: NotConfigured (http.timeout_ms), Timeout/Unreachable, по HTTP-статусу.
pub async fn post(
    ctx: &Ctx,
    url: &str,
    body: &Value,
    headers: &[(&str, &str)],
    source: &str,
) -> GatewayResult<String> {
    // Два срока (gateway/http.rs): до заголовков — http.timeout_ms, тело (пачка Multicall3 — до ~1 МБ) дочитывается,
    // пока данные идут чаще http.body_idle_ms.
    let lim = ctx.limits()?;
    let mut req = ctx.http.post(url).json(body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let (status, text) = http::exchange(req, lim)
        .await
        .map_err(|f| errors::from_fail(&f, &lim, source))?;
    if !status.is_success() {
        // Тело JSON-RPC с ошибкой при не-2xx (так отвечают некоторые каналы) — разбираем его первым.
        if let Err(e) = jsonrpc::parse_text(&text, source, Op::Read)
            && e.code != crate::gateway::errors::ErrorCode::BadResponse
        {
            return Err(e);
        }
        return Err(errors::from_http(status.as_u16(), source, &text));
    }
    Ok(text)
}
