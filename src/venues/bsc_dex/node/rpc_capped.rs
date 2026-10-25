//! Файл: venues/bsc_dex/node/rpc_capped.rs
//! Что делает: один запрос JSON-RPC к узлу HTTP со своими пределами ответа (решение 68): общий срок обмена и размер
//! тела задаёт вызывающий (квитанции блока — bsc.receipts_total_ms, bsc.receipts_max_body_bytes), а не общие
//! http.total_ms / http.max_body_bytes. Превышение предела — отдельный итог (не ошибка узла): вызывающий пишет
//! понятную причину и повторяет. Темп — та же очередь узла (pace.rs), сбой основного узла — резервный (failover.rs).
//! Кто вызывает: node/receipts_io.rs.
//! От чего зависит: rpc.rs (status_ok, SOURCE), pace.rs, failover.rs, jsonrpc.rs, gateway/http.rs.
//! Настройки: bsc.node_http_url (+ резервный), http.timeout_ms, http.body_idle_ms — через ctx; пределы — у вызывающего.
//!
//! Транспорт — как у rpc.rs (HTTP POST, JSON-RPC 2.0, https://www.jsonrpc.org/specification, разделы 4–5).

use serde::de::DeserializeOwned;
use serde_json::Value;

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
