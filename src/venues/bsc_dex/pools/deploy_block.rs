//! Файл: venues/bsc_dex/pools/deploy_block.rs
//! Что делает: блок, с которого читать историю событий Initialize вида пула V4, — блок развёртывания менеджера
//! пулов (CLPoolManager / BinPoolManager / PoolManager). В официальной документации PancakeSwap и Uniswap блоков
//! развёртывания нет, поэтому блок находится по самой сети: бинарный поиск первого блока, на котором у адреса
//! есть код (eth_getCode). Найденный блок хранится в SQLite (storage/venue_state.rs) и больше не ищется;
//! другой адрес менеджера — другой ключ, поиск заново.
//! Кто вызывает: pools/discover.rs (история V4), connection.rs («Проверить подключение»: доступ к истории).
//! От чего зависит: ctx.rs, node/rpc.rs, node/call.rs (номер блока), storage/venue_state.rs.
//! Настройки: узел — bsc.node_http_url (узлу нужен доступ к состоянию на старых блоках — архивный узел).
//!
//! eth_getCode(address, block) → bytes: «Returns code at a given address» на заданном блоке —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/state.yaml ;
//! eth_blockNumber — там же, src/eth/block.yaml. Состояние после блока N содержит контракт, созданный в блоке N,
//! поэтому первый блок с кодом — блок развёртывания: события Initialize раньше него невозможны.

use alloy_primitives::{Address, Bytes, U64};
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;
use crate::storage::venue_state;

use super::super::ctx::{Ctx, ID};
use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::families::Family;

/// Вид записи в venue_state.
const KIND: &str = "deploy_block";

/// Ключ записи: вид пула и адрес менеджера. Ошибок нет.
fn key(f: &Family, manager: Address) -> String {
    format!("{}:{manager:#x}", f.id)
}

/// Сохранённый блок развёртывания. None — ещё не искали (или запись не прочитана — тогда ищем заново). Ошибок нет.
pub async fn stored(ctx: &Ctx, f: &Family, manager: Address) -> Option<u64> {
    let body = venue_state::load(ctx.journal.db(), ID, KIND, &key(f, manager)).await;
    body.ok().flatten().and_then(|b| b.parse().ok())
}

/// Блок развёртывания менеджера: из SQLite, иначе бинарный поиск по eth_getCode и запись в SQLite.
/// Принимает: вид пула, адрес менеджера. Возвращает: номер блока.
/// Ошибки: NotConfigured (по адресу нет контракта), BadResponse с пояснением (узел не отдаёт старое состояние),
/// сеть узла. Сбой записи в SQLite — только в журнал (блок верный, в следующий раз найдётся заново).
pub async fn get(ctx: &Ctx, f: &Family, manager: Address) -> GatewayResult<u64> {
    if let Some(b) = stored(ctx, f, manager).await {
        return Ok(b);
    }
    let msg = format!(
        "{}: ищу блок развёртывания {manager} (eth_getCode)",
        f.title
    );
    ctx.log(Level::Info, "dex.scan", msg, None).await;
    let head: U64 = rpc::call(ctx, "eth_blockNumber", json!([])).await?;
    let block = search(ctx, manager, head.to()).await?;
    let saved = venue_state::save(
        ctx.journal.db(),
        ID,
        KIND,
        &key(f, manager),
        block.to_string(),
    )
    .await;
    let (level, msg) = match saved {
        Ok(()) => (
            Level::Info,
            format!(
                "{}: менеджер {manager} развёрнут в блоке {block} — сохранено",
                f.title
            ),
        ),
        Err(e) => (
            Level::Error,
            format!(
                "{}: блок развёртывания {block} не сохранён в SQLite: {e}",
                f.title
            ),
        ),
    };
    ctx.log(level, "dex.scan", msg, None).await;
    Ok(block)
}

/// Первый блок, на котором у адреса есть код. Принимает: адрес, текущий блок.
/// Ошибки: NotConfigured — кода нет и на текущем блоке; ошибки has_code.
pub async fn search(ctx: &Ctx, addr: Address, head: u64) -> GatewayResult<u64> {
    if !has_code(ctx, addr, head).await? {
        let m =
            format!("по адресу {addr} нет контракта на блоке {head} — проверьте dex.pool_manager");
        return Err(ctx.err(ErrorCode::NotConfigured, m));
    }
    let (mut lo, mut hi) = (0u64, head);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if has_code(ctx, addr, mid).await? {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(lo)
}

/// Есть ли код у адреса на блоке.
/// Ошибки: узел ответил ошибкой — BadResponse с пояснением «нет доступа к истории состояния» и текстом узла;
/// сеть и HTTP — как есть.
pub async fn has_code(ctx: &Ctx, addr: Address, block: u64) -> GatewayResult<bool> {
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, block_tag(block)]))
        .await
        .map_err(|mut e| {
            if e.code == ErrorCode::BadResponse {
                e.message = no_history(block, &e.message);
            }
            e
        })?;
    Ok(!code.is_empty())
}

/// Текст «узел не отдаёт состояние на старом блоке». Принимает: блок, ответ узла. Ошибок нет.
pub fn no_history(block: u64, node: &str) -> String {
    format!(
        "узел не отдаёт состояние на блоке {block} (eth_getCode): {node} — для поиска пулов V4 нужен узел \
         с доступом к истории состояния (архивный)"
    )
}
