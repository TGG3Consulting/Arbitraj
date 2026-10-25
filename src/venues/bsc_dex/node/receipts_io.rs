//! Файл: venues/bsc_dex/node/receipts_io.rs
//! Что делает: запросы ленты квитанций к узлу HTTP (решение 68): заголовок блока (hash, parentHash, хеши транзакций)
//! и квитанции блока со своими пределами ответа (bsc.receipts_max_body_bytes, bsc.receipts_total_ms — только для
//! квитанций) и проверкой полноты: квитанций столько же, сколько транзакций в заголовке, по порядку тех же хешей, все —
//! этого блока. Неполные, чужого хеша, больше предела — повтор того же блока (блоки не пропускаются).
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: rpc.rs, rpc_capped.rs (очередь узла, класс задачи), receipts_chain.rs (complete), ctx.rs.
//! Настройки: bsc.receipts_max_body_bytes, bsc.receipts_total_ms [вся система]; http.timeout_ms, http.body_idle_ms.
//!
//! eth_getBlockByNumber (false — transactions как хеши) — execution-apis src/eth/block.yaml; eth_getBlockReceipts —
//! https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ .

use std::time::Duration;

use alloy_primitives::B256;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::gateway::http::{Fail as HttpFail, Limits};

use super::super::ctx::Ctx;
use super::receipts_chain::complete;
use super::rpc;
use super::rpc_capped::{self, Capped};

/// Заголовок блока: только нужные поля.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Header {
    pub(super) hash: B256,
    pub(super) parent_hash: B256,
    /// Хеши транзакций блока (eth_getBlockByNumber с false).
    #[serde(default)]
    pub(super) transactions: Vec<B256>,
}

/// Чем кончился шаг.
pub(super) enum Fail {
    /// Повторить тот же блок через bsc.receipts_retry_ms.
    Retry(String),
    /// Обрыв ленты.
    Break(String),
}

/// Заголовок блока. Ошибки: Retry — узел не ответил или блока ещё нет.
pub(super) async fn header(ctx: &Ctx, n: u64) -> Result<Header, Fail> {
    let h: Option<Header> = rpc::call(
        ctx,
        "eth_getBlockByNumber",
        json!([format!("0x{n:x}"), false]),
    )
    .await
    .map_err(|e| Fail::Retry(e.message))?;
    h.ok_or_else(|| Fail::Retry(format!("узел ещё не отдал блок {n}")))
}

/// Квитанции блока `h` (номер `n`): полные и этого блока. Ошибки: Break — не заданы пределы; Retry — узел не
/// ответил, блока ещё нет, квитанции неполные или другого хеша (реорг идёт), ответ больше предела.
pub(super) async fn receipts(ctx: &Ctx, n: u64, h: &Header) -> Result<Vec<Value>, Fail> {
    let lim = limits(ctx)?;
    let got: Capped<Option<Vec<Value>>> = rpc_capped::call(
        ctx,
        "eth_getBlockReceipts",
        json!([format!("0x{n:x}")]),
        lim,
    )
    .await
    .map_err(|e| Fail::Retry(e.message))?;
    let r = match got {
        Capped::Got(r) => r,
        Capped::Over(f) => return Err(Fail::Retry(over(n, &f, &lim))),
    };
    let r = r.ok_or_else(|| Fail::Retry(format!("узел ещё не отдал квитанции блока {n}")))?;
    complete(n, h.hash, &h.transactions, &r).map_err(Fail::Retry)?;
    Ok(r)
}

/// Пределы запроса квитанций: срок до заголовков и тишина тела — общие (http.timeout_ms, http.body_idle_ms), общий
/// срок и размер — свои. Ошибки: Break — настройка не задана.
fn limits(ctx: &Ctx) -> Result<Limits, Fail> {
    let base = ctx.limits().map_err(|e| Fail::Break(e.message))?;
    let set = |name: &str| ctx.int(name).map_err(|e| Fail::Break(e.message));
    Ok(Limits {
        total: Duration::from_millis(set("bsc.receipts_total_ms")?),
        max_bytes: usize::try_from(set("bsc.receipts_max_body_bytes")?).unwrap_or(usize::MAX),
        ..base
    })
}

/// Текст «квитанции больше предела». Ошибок нет.
fn over(n: u64, f: &HttpFail, lim: &Limits) -> String {
    match f {
        HttpFail::TooBig { got } => format!(
            "блок {n}: квитанции больше предела bsc.receipts_max_body_bytes ({} байт), получено {got} байт",
            lim.max_bytes
        ),
        HttpFail::Total { got } => format!(
            "блок {n}: квитанции больше предела bsc.receipts_total_ms ({} мс), получено {got} байт",
            lim.total.as_millis()
        ),
        other => format!("блок {n}: {}", other.text(lim)),
    }
}
