//! Файл: venues/bsc_dex/pools/scan.rs
//! Что делает: чтение истории событий (eth_getLogs) кусками по dex.logs_block_batch блоков — для поиска пулов
//! по событиям Initialize V4. Ход — в журнал не чаще раза в dex.scan_progress_s секунд (прочитано, осталось блоков);
//! итог — если чтение было долгим (был ход) или нашлись события: короткое дочитывание без событий журнал не засоряет.
//! Кто вызывает: pools/discover_v4.rs, pools/history_v4.rs (по куску, со своим ходом).
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

use crate::gateway::errors::GatewayResult;
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
    let batch = batch_size(ctx)?;
    let mut prog = Progress::new(ctx, what, from, to)?;
    let mut out = Vec::new();
    let mut start = from;
    while start <= to {
        let end = to.min(start.saturating_add(batch - 1));
        out.append(&mut one(ctx, address, &topics, start, end, what).await?);
        start = end + 1;
        prog.step(ctx, end, out.len()).await;
    }
    prog.finish(ctx, out.len()).await;
    Ok(out)
}

/// Размер куска: dex.logs_block_batch. Ошибки: NotConfigured (не задан или 0).
pub fn batch_size(ctx: &Ctx) -> GatewayResult<u64> {
    match ctx.int("dex.logs_block_batch")? {
        0 => Err(ctx.not_set(
            "dex.logs_block_batch",
            "dex.logs_block_batch должен быть больше нуля",
        )),
        n => Ok(n),
    }
}

/// Один запрос eth_getLogs за блоки [start, end].
/// Ошибки: узел (текст как есть, с блоками и подсказкой про dex.logs_block_batch), BadResponse.
pub async fn one(
    ctx: &Ctx,
    address: Address,
    topics: &Value,
    start: u64,
    end: u64,
    what: &str,
) -> GatewayResult<Vec<Log>> {
    let filter = json!({
        "address": address,
        "topics": topics,
        "fromBlock": block_tag(start),
        "toBlock": block_tag(end),
    });
    rpc::call(ctx, "eth_getLogs", json!([filter]))
        .await
        .map_err(|mut e| {
            e.message = format!(
                "{what}: блоки {start}–{end}: {} (лимит провайдера на диапазон eth_getLogs — уменьшите dex.logs_block_batch)",
                e.message
            );
            e
        })
}

/// Ход чтения истории для журнала: не чаще раза в dex.scan_progress_s; итог — только после долгого чтения
/// (был ход) или если нашлись события.
pub struct Progress {
    what: String,
    from: u64,
    to: u64,
    every: Duration,
    started: Instant,
    last: Instant,
    told: bool,
    /// Сколько пулов подключено по ходу чтения (история V4) — в той же строке хода, без записи на каждый кусок.
    pub attached: usize,
}

impl Progress {
    /// Начало чтения [from, to]. Ошибки: NotConfigured (dex.scan_progress_s).
    pub fn new(ctx: &Ctx, what: &str, from: u64, to: u64) -> GatewayResult<Self> {
        let now = Instant::now();
        Ok(Self {
            what: what.to_string(),
            from,
            to,
            every: Duration::from_secs(ctx.int("dex.scan_progress_s")?),
            started: now,
            last: now,
            told: false,
            attached: 0,
        })
    }

    /// Пора ли писать ход чтения (прошло dex.scan_progress_s с прошлой записи и чтение не кончилось). Ошибок нет.
    pub fn due(&self, done: u64) -> bool {
        done < self.to && self.last.elapsed() >= self.every
    }

    /// Прочитано до блока done, событий всего events: запись хода, если пора. Ошибок нет.
    pub async fn step(&mut self, ctx: &Ctx, done: u64, events: usize) {
        if done < self.to && self.last.elapsed() >= self.every {
            self.last = Instant::now();
            self.told = true;
            let msg = progress(&self.what, self.from, done, self.to, events) + &self.pools();
            ctx.log(Level::Info, "dex.scan", msg, None).await;
        }
    }

    /// Итог чтения. Ошибок нет.
    pub async fn finish(&self, ctx: &Ctx, events: usize) {
        if self.told || events > 0 {
            let msg = format!(
                "{}: прочитаны блоки {}–{} за {} с, событий {events}{}",
                self.what,
                self.from,
                self.to,
                self.started.elapsed().as_secs(),
                self.pools()
            );
            ctx.log(Level::Info, "dex.scan", msg, None).await;
        }
    }
}

impl Progress {
    /// «, подключено пулов M» — если подключались. Ошибок нет.
    fn pools(&self) -> String {
        if self.attached > 0 {
            format!(", подключено пулов {}", self.attached)
        } else {
            String::new()
        }
    }
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
