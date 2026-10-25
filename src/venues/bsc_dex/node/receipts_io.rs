//! Файл: venues/bsc_dex/node/receipts_io.rs
//! Что делает: запросы ленты квитанций к узлу HTTP (решение 68) и раздача логов подпискам: заголовок блока
//! (hash, parentHash), квитанции блока, раздача по фильтрам (receipts_chain.rs route), обрыв ленты.
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: rpc.rs (очередь узла, класс задачи), receipts.rs (Inner), receipts_chain.rs.
//! Настройки: нет.
//!
//! eth_getBlockByNumber — execution-apis src/eth/block.yaml; eth_getBlockReceipts —
//! https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ .

use std::sync::Arc;

use alloy_primitives::B256;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::ctx::Ctx;
use super::receipts::{Inner, SOURCE};
use super::receipts_chain::route;
use super::rpc;

/// Заголовок блока: только нужные поля.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Header {
    pub(super) hash: B256,
    pub(super) parent_hash: B256,
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

/// Раздаёт логи подпискам; закрытые приёмники снимаются. Возвращает: логи, подошедшие хоть одной. Ошибок нет.
pub(super) fn deliver(inner: &Arc<Inner>, logs: &[Value]) -> Vec<Value> {
    let mut s = inner.state.lock().unwrap_or_else(|p| p.into_inner());
    let ids: Vec<String> = s.subs.keys().cloned().collect();
    let filters: Vec<_> = ids.iter().map(|id| &s.subs[id].0).collect();
    let (per, any) = route(&filters, logs);
    let mut gone = Vec::new();
    for (id, idx) in ids.iter().zip(per) {
        let tx = &s.subs[id].1;
        for i in idx {
            let Ok(raw) = serde_json::value::to_raw_value(&logs[i]) else {
                continue;
            };
            if tx.send(raw).is_err() {
                gone.push(id.clone());
                break;
            }
        }
    }
    for id in gone {
        s.subs.remove(&id);
    }
    any
}

/// Обрыв ленты: новое поколение, все подписки закрыты (их приёмники видят конец), последнего блока нет. Ошибок нет.
pub(super) fn cut(inner: &Arc<Inner>, why: &str) {
    let mut s = inner.state.lock().unwrap_or_else(|p| p.into_inner());
    s.generation += 1;
    s.broken = Some(format!("{SOURCE}: {why}"));
    s.subs.clear();
    inner.last.send_replace(None);
}

/// Квитанции блока; все — того же хеша, что заголовок. Ошибки: Retry — узел не ответил, блока ещё нет или квитанции
/// другого хеша (реорг идёт).
pub(super) async fn receipts(ctx: &Ctx, n: u64, hash: B256) -> Result<Vec<Value>, Fail> {
    let r: Option<Vec<Value>> = rpc::call(ctx, "eth_getBlockReceipts", json!([format!("0x{n:x}")]))
        .await
        .map_err(|e| Fail::Retry(e.message))?;
    let r = r.ok_or_else(|| Fail::Retry(format!("узел ещё не отдал квитанции блока {n}")))?;
    let other = |x: &Value| {
        x.get("blockHash")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<B256>().ok())
            != Some(hash)
    };
    if r.iter().any(other) {
        return Err(Fail::Retry(format!(
            "квитанции блока {n} — другого хеша (реорг идёт)"
        )));
    }
    Ok(r)
}
