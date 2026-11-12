//! Файл: venues/bsc_dex/check/quoter_block.rs
//! Что делает: решение 261, часть 2 (Тигран 12.11.2026) — номер блока, на котором состояние пула в памяти бота равно
//! состоянию сети: на нём периодическая сверка зовёт квотер (а не на «latest»), чтобы расхождение было ошибкой
//! математики, а не сменой блока (блок BSC ~450 мс).
//! Факт кода (pools/registry.rs Entry): у пула хранится позиция последнего учтённого события — synced_block и
//! synced_log (logIndex; None — состояние прочитано на конце synced_block целиком, pools/load.rs). Слушатель применяет
//! события по порядку (blockNumber, logIndex), отметки «блок разобран целиком» у пула нет. Поэтому:
//! - пул, перечитываемый на каждом блоке (pools/per_block.rs needs: Lista, WooFi, Infinity Stable — состояние меняет
//!   оракул без событий): блок = synced_block, только если состояние прочитано на нём целиком (synced_log = None);
//! - остальные: eth_getLogs пула (тот же фильтр, что у подписки, — pools/group.rs filter) с synced_block до головы узла
//!   HTTP (node/heads.rs); событий после учтённой позиции нет — состояние в памяти равно состоянию сети на голове,
//!   сверка — на голове; есть — память ещё не догнала сеть (или позиция — середина блока), «не сверено (блок)».
//!
//! Блок не определён (голова неизвестна, узел HTTP позади, узел не ответил) — «не сверено (блок)» с причиной: это не
//! «математика неверна», пул не исключается и не паузится (check/quoter_cycle.rs).
//! Кто вызывает: check/quoter_compare.rs.
//! От чего зависит: node/rpc.rs, node/subs.rs, node/call.rs (block_tag), node/heads.rs, pools/events.rs (topics),
//! pools/per_block.rs (needs), pools/registry.rs.
//! Настройки: нет (узел — через node/rpc.rs).
//!
//! eth_getLogs(filter {fromBlock, toBlock, address, topics}) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (eth_getLogs);
//! порядок событий — (blockNumber, logIndex), Log.logIndex — «index of the log in the block» (src/schemas/receipt.yaml).

use alloy::rpc::types::Log;
use serde_json::{Value, json};

use crate::core::types::PoolRef;

use super::super::node::{call, rpc, subs};
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::pools::{events, per_block};

/// Начало причины «не сверено (блок)» — по ней сторона не исключается и не паузится (check/quoter_cycle.rs).
pub const NOT_CHECKED: &str = "не сверено (блок)";

/// Блок сверки по позиции пула и событиям пула после неё (чистая, тестируется на записанных логах).
/// Принимает: synced_block, synced_log, перечитывается ли пул на каждом блоке, позиции (блок, logIndex) событий пула
/// из eth_getLogs (без removed), голову узла HTTP. Возвращает: блок сверки. Ошибки (текст): блок не определён.
pub fn settle(
    block: u64,
    log: Option<u64>,
    every_block: bool,
    seen: &[(u64, u64)],
    head: u64,
) -> Result<u64, String> {
    if block == 0 {
        return Err("блок состояния пула не известен".into());
    }
    if every_block {
        return match log {
            None => Ok(block),
            Some(i) => Err(format!(
                "пул перечитывается на каждом блоке, а состояние — после события logIndex {i} блока {block}"
            )),
        };
    }
    if head < block {
        return Err(format!(
            "узел HTTP на блоке {head}, состояние пула — на блоке {block}"
        ));
    }
    // Событие после учтённой позиции: позже блока или в том же блоке после logIndex (None — блок учтён целиком).
    let later = seen
        .iter()
        .find(|(b, i)| *b > block || (*b == block && log.is_some_and(|x| *i > x)));
    match later {
        Some((b, i)) => Err(format!(
            "у пула есть событие блока {b} (logIndex {i}) после учтённого (блок {block}, logIndex {}) — состояние в \
             памяти ещё не догнало сеть",
            log.map_or("весь блок".to_string(), |x| x.to_string())
        )),
        None => Ok(head),
    }
}

/// Блок сверки пула (см. шапку). Ошибки (текст): блок не определён — причина «не сверено (блок)».
pub async fn block(p: &Pools, e: &Entry) -> Result<u64, String> {
    let every = per_block::needs(e.family);
    if every || e.synced_block == 0 {
        return settle(e.synced_block, e.synced_log, every, &[], e.synced_block);
    }
    let head = p
        .heads
        .latest()
        .ok_or("номер головы узла HTTP ещё не известен (опрос eth_blockNumber)")?
        .number;
    if head < e.synced_block {
        return settle(e.synced_block, e.synced_log, false, &[], head);
    }
    let mut f = filter(e);
    f["fromBlock"] = json!(call::block_tag(e.synced_block));
    f["toBlock"] = json!(call::block_tag(head));
    let logs: Vec<Log> = rpc::call(&p.ctx, "eth_getLogs", json!([f]))
        .await
        .map_err(|err| {
            format!(
                "eth_getLogs пула блоков {}–{head}: {}",
                e.synced_block, err.message
            )
        })?;
    let mut seen = Vec::with_capacity(logs.len());
    for l in logs.iter().filter(|l| !l.removed) {
        let (Some(b), Some(i)) = (l.block_number, l.log_index) else {
            return Err("eth_getLogs вернул событие без blockNumber / logIndex".into());
        };
        seen.push((b, i));
    }
    settle(e.synced_block, e.synced_log, false, &seen, head)
}

/// Фильтр событий одного пула — как у подписки пачки (pools/group.rs filter, Topics::Full). Ошибок нет.
fn filter(e: &Entry) -> Value {
    let topic0 = events::topics(e.family);
    match e.pool.reference {
        PoolRef::Contract(a) | PoolRef::Member { contract: a, .. } => {
            subs::by_address(&[a], &topic0)
        }
        PoolRef::Singleton { manager, id } => subs::by_pool_id(manager, &[id], &topic0),
    }
}
