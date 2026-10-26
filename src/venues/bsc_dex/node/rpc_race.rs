//! Файл: venues/bsc_dex/node/rpc_race.rs
//! Что делает: гонка одной пачки JSON-RPC на все узлы HTTP (основной и резервный bsc.node_http_url_backup) — решение 71:
//! повтор тяжёлого блока ленты квитанций, на котором стоит конвейер. Побеждает первый ответ, ПРОШЕДШИЙ проверку
//! вызывающего (для ленты — заголовок есть и квитанции полные), а не первый HTTP 200: отстающий узел может быстро
//! ответить null или неполными квитанциями.
//! Кто вызывает: node/receipts_io.rs (fetch_block); tests/bsc_dex_receipts_pipe.rs (first_accepted на записях).
//! От чего зависит: rpc_capped.rs (Capped, split), rpc.rs (status_ok, SOURCE), pace.rs, failover.rs, gateway/http.rs.
//! Настройки: bsc.node_http_url, bsc.node_http_url_backup — через failover.rs.

use serde_json::Value;
use serde_json::value::RawValue;

use crate::gateway::errors::GatewayResult;
use crate::gateway::http::{self, Fail, Limits};

use super::super::ctx::Ctx;
use super::super::errors;
use super::failover::Link;
use super::rpc::{SOURCE, status_ok};
use super::rpc_capped::{Capped, split};
use super::{jsonrpc, pace};

/// Чем кончилась гонка без принятого ответа.
pub enum Lost {
    /// Ответ больше предела (последний такой).
    Over(Fail),
    /// Ответы пришли, но ни один не прошёл проверку вызывающего (последняя причина: null, неполные квитанции…).
    Rejected(String),
    /// Ошибка связи или узла (последняя).
    Failed(crate::gateway::errors::GatewayError),
}

/// Та же пачка СРАЗУ на все узлы (основной и резервный bsc.node_http_url_backup) — первый ответ, ПРОШЕДШИЙ проверку
/// `accept` (решение 71: повтор тяжёлого блока ленты, на котором стоит конвейер). Отстающий узел может быстро ответить
/// null или неполными квитанциями — такой ответ не выигрывает, ждём остальных. Каждый запрос — в очереди узла со своим
/// весом. Принимает: вызовы, пределы, проверку итогов пачки. Возвращает: принятое; ни одного — Lost.
pub async fn batch_race<T>(
    ctx: &Ctx,
    calls: &[(&str, Value)],
    lim: Limits,
    accept: impl Fn(Vec<GatewayResult<Box<RawValue>>>) -> Result<T, String>,
) -> Result<T, Lost> {
    let urls = ctx.failover.urls(ctx, Link::Http).map_err(Lost::Failed)?;
    let body = Value::Array(
        calls
            .iter()
            .enumerate()
            .map(|(i, (m, p))| jsonrpc::request(i as u64, m, p))
            .collect(),
    );
    let runs: Vec<_> = urls
        .iter()
        .map(|(_, url)| {
            let body = &body;
            async move { pace::run(ctx, calls.len(), || one(ctx, url.as_str(), body, lim)).await }
        })
        .collect();
    first_accepted(runs, calls.len(), accept).await
}

/// Первый ответ из `runs`, прошедший `accept` (без сети — проверяется на записанных ответах). Не прошёл, больше
/// предела, ошибка — ждём остальных. Принимает: запросы, число вызовов пачки, проверку. Возвращает: принятое или
/// Lost (непринятый ответ важнее ошибки связи: в нём причина). Ошибок кроме Lost нет.
pub async fn first_accepted<T, F>(
    runs: Vec<F>,
    n: usize,
    accept: impl Fn(Vec<GatewayResult<Box<RawValue>>>) -> Result<T, String>,
) -> Result<T, Lost>
where
    F: std::future::Future<Output = GatewayResult<Capped<String>>>,
{
    let mut runs: futures_util::stream::FuturesUnordered<F> = runs.into_iter().collect();
    let (mut over, mut rejected, mut failed) = (None, None, None);
    while let Some(res) = futures_util::StreamExt::next(&mut runs).await {
        match res {
            Ok(Capped::Got(text)) => match split(&text, n).map_err(|e| e.message).and_then(&accept)
            {
                Ok(t) => return Ok(t),
                Err(why) => rejected = Some(why),
            },
            Ok(Capped::Over(f)) => over = Some(f),
            Err(e) => failed = Some(e),
        }
    }
    Err(match (rejected, over, failed) {
        (Some(r), _, _) => Lost::Rejected(r),
        (None, Some(f), _) => Lost::Over(f),
        (None, None, Some(e)) => Lost::Failed(e),
        (None, None, None) => Lost::Failed(errors::bad("нет адреса узла", SOURCE, "")),
    })
}

/// Один узел: обмен с пределами. Ошибки: связь, HTTP; превышение предела — Over.
async fn one(ctx: &Ctx, url: &str, body: &Value, lim: Limits) -> GatewayResult<Capped<String>> {
    match http::exchange(ctx.http.post(url).json(body), lim).await {
        Ok((status, text)) => status_ok(status, text, SOURCE).map(Capped::Got),
        Err(f @ (Fail::TooBig { .. } | Fail::Total { .. })) => Ok(Capped::Over(f)),
        Err(f) => Err(errors::from_fail(&f, &lim, SOURCE)),
    }
}
