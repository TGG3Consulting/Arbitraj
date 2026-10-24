//! Файл: venues/bsc_dex/pools/scan.rs
//! Что делает: чтение истории событий (eth_getLogs) кусками по dex.logs_block_batch блоков — для поиска пулов
//! по событиям Initialize V4. Ход — в журнал не чаще раза в dex.scan_progress_s секунд (прочитано, осталось блоков);
//! итог — если чтение было долгим (был ход) или нашлись события: короткое дочитывание без событий журнал не засоряет.
//! Кто вызывает: pools/discover_v4.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, node/call.rs (номер блока в hex).
//! Настройки: dex.logs_block_batch, dex.scan_progress_s [bsc_dex]; узел — bsc.node_http_url (у публичных узлов BNB Chain eth_getLogs
//! выключен — research/flow/bsc_node.md, раздел 1: нужен сторонний провайдер).
//!
//! eth_getLogs(filter {address, topics, fromBlock, toBlock}) —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/filter.yaml ;
//! topics: позиция = И, массив внутри позиции = ИЛИ, null = любое; у узла BSC до 1000 вариантов в позиции —
//! https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/eth/filters/api.go#L62-L69 .

use std::time::{Duration, Instant};

use alloy::rpc::types::Log;
use alloy_primitives::Address;
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::rpc;

/// События контракта за блоки [from, to] кусками.
/// Принимает: адрес контракта, topics (массив JSON), блоки, подпись для журнала.
/// Возвращает: все логи по порядку. Ошибки: NotConfigured (dex.logs_block_batch, dex.scan_progress_s),
/// узел (лимиты провайдера — его текст как есть, с подсказкой про dex.logs_block_batch), BadResponse.
pub async fn logs(
    ctx: &Ctx,
    address: Address,
    topics: Value,
    from: u64,
    to: u64,
    what: &str,
) -> GatewayResult<Vec<Log>> {
    let batch = ctx.int("dex.logs_block_batch")?;
    if batch == 0 {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "dex.logs_block_batch должен быть больше нуля",
        ));
    }
    let every = Duration::from_secs(ctx.int("dex.scan_progress_s")?);
    let started = Instant::now();
    let mut last = started;
    let mut told = false;
    let mut out = Vec::new();
    let mut start = from;
    while start <= to {
        let end = to.min(start.saturating_add(batch - 1));
        let filter = json!({
            "address": address,
            "topics": topics,
            "fromBlock": block_tag(start),
            "toBlock": block_tag(end),
        });
        let mut part: Vec<Log> = rpc::call(ctx, "eth_getLogs", json!([filter]))
            .await
            .map_err(|mut e| {
                e.message = format!(
                    "{what}: блоки {start}–{end}: {} (лимит провайдера на диапазон eth_getLogs — уменьшите dex.logs_block_batch)",
                    e.message
                );
                e
            })?;
        out.append(&mut part);
        start = end + 1;
        if start <= to && last.elapsed() >= every {
            last = Instant::now();
            told = true;
            let msg = progress(what, from, end, to, out.len());
            ctx.log(Level::Info, "dex.scan", msg, None).await;
        }
    }
    if told || !out.is_empty() {
        let msg = format!(
            "{what}: прочитаны блоки {from}–{to} за {} с, событий {}",
            started.elapsed().as_secs(),
            out.len()
        );
        ctx.log(Level::Info, "dex.scan", msg, None).await;
    }
    Ok(out)
}

/// Текст хода чтения. Принимает: подпись, первый блок, последний прочитанный, последний нужный, событий.
/// Возвращает: строку для журнала. Ошибок нет.
fn progress(what: &str, from: u64, done: u64, to: u64, events: usize) -> String {
    let total = to - from + 1;
    let read = done - from + 1;
    format!(
        "{what}: прочитано {read} из {total} блоков ({}%), осталось {} блоков, событий {events}",
        read * 100 / total,
        to - done
    )
}
