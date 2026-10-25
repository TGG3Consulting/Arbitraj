//! Файл: venues/bsc_dex/node/rpc_capped.rs
//! Что делает: один запрос JSON-RPC к узлу HTTP со своими пределами ответа (решение 68): общий срок обмена и размер
//! тела задаёт вызывающий (квитанции блока — bsc.receipts_total_ms, bsc.receipts_max_body_bytes), а не общие
//! http.total_ms / http.max_body_bytes. Превышение предела — отдельный итог (не ошибка узла): вызывающий пишет
//! понятную причину и повторяет. Темп — та же очередь узла (pace.rs), сбой основного узла — резервный (failover.rs).
//! Пачка (batch) — тоже: заголовок и квитанции блока одним HTTP (решение 69; публичный узел BNB Chain batch
//! принимает — запись tests/data/receipts_batch_2026-10-25). Ответы — по id, порядок в массиве не важен. Ошибка
//! отдельного вызова внутри пачки (в том числе «слишком много запросов» от узла с HTTP 200) — итог этого вызова, а не
//! всей пачки: правило шлюза RateLimited (повтор в pace.rs) срабатывает только на ответ всей пачки 429; лента
//! квитанций такой итог повторяет через bsc.receipts_retry_ms (receipts_io.rs, Fail::Retry).
//! Кто вызывает: node/receipts_io.rs; tests/bsc_dex_receipts_pipe.rs (split на записанном ответе).
//! От чего зависит: rpc.rs (status_ok, SOURCE), pace.rs, failover.rs, jsonrpc.rs, gateway/http.rs.
//! Настройки: bsc.node_http_url (+ резервный), http.timeout_ms, http.body_idle_ms — через ctx; пределы — у вызывающего.
//!
//! Транспорт — как у rpc.rs (HTTP POST, JSON-RPC 2.0, https://www.jsonrpc.org/specification, разделы 4–5).

use alloy::rpc::json_rpc::Response;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::gateway::errors::GatewayResult;
use crate::gateway::http::{self, Fail, Limits};

use super::super::ctx::Ctx;
use super::super::errors::{self, Op};
use super::failover::{self, Link, Node};
use super::rpc::{SOURCE, status_ok};
use super::{jsonrpc, pace};

/// Итог запроса с пределом: ответ или превышение предела (TooBig / Total — сколько получено).
pub enum Capped<T> {
    Got(T),
    Over(Fail),
}

/// Запрос с пределами `lim`. Принимает: метод, параметры, пределы. Возвращает: Got(result) или Over(сбой предела).
/// Ошибки: как у rpc::call (настройки, связь, ответ-ошибка узла, BadResponse).
pub async fn call<T: DeserializeOwned>(
    ctx: &Ctx,
    method: &str,
    params: Value,
    lim: Limits,
) -> GatewayResult<Capped<T>> {
    let body = jsonrpc::request(1, method, &params);
    match pace::run(ctx, 1, || to_node(ctx, &body, lim)).await? {
        Capped::Over(f) => Ok(Capped::Over(f)),
        Capped::Got(text) => {
            let raw = jsonrpc::parse_text(&text, SOURCE, Op::Read)?;
            Ok(Capped::Got(jsonrpc::decode(&raw, SOURCE)?))
        }
    }
}

/// Пачка вызовов (JSON-RPC batch) одним HTTP с пределами `lim` (решение 69: заголовок и квитанции блока вместе). В
/// очереди узла — вес по числу вызовов (bsc.node_http_rps считает вызовы). Принимает: (метод, параметры), пределы.
/// Возвращает: Got(итоги по порядку вызовов, у каждого свой Ok/Err) или Over. Ошибки: связь, HTTP, ответ не массив.
pub async fn batch(
    ctx: &Ctx,
    calls: &[(&str, Value)],
    lim: Limits,
) -> GatewayResult<Capped<Vec<GatewayResult<Box<RawValue>>>>> {
    let body = Value::Array(
        calls
            .iter()
            .enumerate()
            .map(|(i, (m, p))| jsonrpc::request(i as u64, m, p))
            .collect(),
    );
    match pace::run(ctx, calls.len(), || to_node(ctx, &body, lim)).await? {
        Capped::Over(f) => Ok(Capped::Over(f)),
        Capped::Got(text) => Ok(Capped::Got(split(&text, calls.len())?)),
    }
}

/// Та же пачка СРАЗУ на все узлы (основной и резервный bsc.node_http_url_backup) — первый полный ответ (решение 71:
/// повтор тяжёлого блока ленты, на котором стоит конвейер; ускорить надо именно его). Каждый запрос — в очереди узла
/// со своим весом. Резервного нет — как batch. Принимает: вызовы, пределы. Возвращает: первый Got; ни одного Got —
/// Over, если он был, иначе последняя ошибка. Ошибки: как у batch.
pub async fn batch_race(
    ctx: &Ctx,
    calls: &[(&str, Value)],
    lim: Limits,
) -> GatewayResult<Capped<Vec<GatewayResult<Box<RawValue>>>>> {
    let urls = ctx.failover.urls(ctx, Link::Http)?;
    if urls.len() < 2 {
        return batch(ctx, calls, lim).await;
    }
    let body = Value::Array(
        calls
            .iter()
            .enumerate()
            .map(|(i, (m, p))| jsonrpc::request(i as u64, m, p))
            .collect(),
    );
    let mut runs: futures_util::stream::FuturesUnordered<_> = urls
        .iter()
        .map(|(_, url)| {
            let body = &body;
            async move { pace::run(ctx, calls.len(), || one(ctx, url.as_str(), body, lim)).await }
        })
        .collect();
    let (mut over, mut last) = (None, None);
    while let Some(res) = futures_util::StreamExt::next(&mut runs).await {
        match res {
            Ok(Capped::Got(text)) => return Ok(Capped::Got(split(&text, calls.len())?)),
            Ok(Capped::Over(f)) => over = Some(f),
            Err(e) => last = Some(e),
        }
    }
    match (over, last) {
        (Some(f), _) => Ok(Capped::Over(f)),
        (None, Some(e)) => Err(e),
        (None, None) => Err(errors::bad("нет адреса узла", SOURCE, "")),
    }
}

/// Один узел: обмен с пределами. Ошибки: связь, HTTP; превышение предела — Over.
async fn one(ctx: &Ctx, url: &str, body: &Value, lim: Limits) -> GatewayResult<Capped<String>> {
    match http::exchange(ctx.http.post(url).json(body), lim).await {
        Ok((status, text)) => status_ok(status, text, SOURCE).map(Capped::Got),
        Err(f @ (Fail::TooBig { .. } | Fail::Total { .. })) => Ok(Capped::Over(f)),
        Err(f) => Err(errors::from_fail(&f, &lim, SOURCE)),
    }
}

/// Ответ на пачку → итоги по id (0 … n−1), как JSON-RPC 2.0 раздел 6 «Batch»: порядок ответов в массиве не важен.
/// Принимает: текст ответа, число вызовов. Возвращает: итоги по порядку вызовов. Ошибки: ответ не массив (узел
/// ответил одной ошибкой на всю пачку — её текст), нет ответа на какой-то id (BadResponse).
pub fn split(text: &str, n: usize) -> GatewayResult<Vec<GatewayResult<Box<RawValue>>>> {
    let list: Vec<Response> =
        serde_json::from_str(text).map_err(|e| {
            match jsonrpc::parse_text(text, SOURCE, Op::Read) {
                Err(err) => err,
                Ok(_) => errors::bad(&format!("ответ на пачку не массив: {e}"), SOURCE, text),
            }
        })?;
    let mut out: Vec<Option<GatewayResult<Box<RawValue>>>> = (0..n).map(|_| None).collect();
    for resp in list {
        let (id, res) = jsonrpc::parse(resp, SOURCE, Op::Read, text);
        if let Some(slot) = id.and_then(|i| out.get_mut(i as usize)) {
            *slot = Some(res);
        }
    }
    out.into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.ok_or_else(|| errors::bad(&format!("нет ответа на запрос {i} пачки"), SOURCE, text))
        })
        .collect()
}

/// Основной узел, при сбое (связь, таймаут, 5xx, 429) — резервный; превышение предела — сразу итогом (размер ответа
/// от узла не зависит). Ошибки: адрес/ключ не заданы; ошибка последнего узла.
async fn to_node(ctx: &Ctx, body: &Value, lim: Limits) -> GatewayResult<Capped<String>> {
    let urls = ctx.failover.urls(ctx, Link::Http)?;
    let n = urls.len();
    let mut last = None;
    for (i, (node, url)) in urls.into_iter().enumerate() {
        let res = match http::exchange(ctx.http.post(url.as_str()).json(body), lim).await {
            Ok((status, text)) => status_ok(status, text, SOURCE).map(Capped::Got),
            Err(f @ (Fail::TooBig { .. } | Fail::Total { .. })) => return Ok(Capped::Over(f)),
            Err(f) => Err(errors::from_fail(&f, &lim, SOURCE)),
        };
        match res {
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
