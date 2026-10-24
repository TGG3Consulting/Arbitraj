//! Файл: venues/bsc_dex/connection_extra.rs
//! Что делает: части «Проверить подключение» DEX для резервного узла и Multicall3: резервный узел HTTP (сеть, блок)
//! и WebSocket (сеть по eth_chainId на соединении) — по своим адресам, мимо переключения (node/failover.rs);
//! Multicall3 — есть ли код по адресу dex.multicall3 (eth_getCode). Не задано — «не задан», это не ошибка.
//! Кто вызывает: connection.rs.
//! От чего зависит: ctx.rs, node/rpc.rs (post), node/ws.rs (Conn::connect), node/jsonrpc.rs, node/multicall.rs.
//! Обёртка нативной монеты (решение 62) — код по адресу dex.native_wrapped и совпадение с WETH() роутеров V2.
//! Настройки: bsc.node_http_url_backup, bsc.node_ws_url_backup, dex.multicall3, dex.native_wrapped; секреты bsc.node_*_key_backup.
//!
//! eth_chainId, eth_blockNumber, eth_getCode — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth

use alloy_primitives::{Bytes, U64};
use serde_json::json;

use crate::settings::read::At;

use super::ctx::Ctx;
use super::errors::Op;
use super::node::failover::{Link, Node};
use super::node::rpc::{self, SOURCE};
use super::node::ws::Conn;
use super::node::{BSC_CHAIN_ID, jsonrpc, multicall};

/// Адрес резервного узла с ключом. None — резервный не задан. Ошибка — текст (ключ не введён и т.п.).
fn backup_url(ctx: &Ctx, link: Link) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    let (url, key) = link.names(Node::Backup);
    if ctx.settings.text(url, At::default()).is_err() {
        return Ok(None);
    }
    ctx.node_url(url, key).map(Some).map_err(|e| e.message)
}

/// Один вызов к резервному узлу HTTP напрямую. Ошибка — текст.
async fn call<T: serde::de::DeserializeOwned>(
    ctx: &Ctx,
    url: &str,
    method: &str,
) -> Result<T, String> {
    let body = jsonrpc::request(1, method, &json!([]));
    let text = rpc::post(ctx, url, &body, &[], SOURCE)
        .await
        .map_err(|e| e.message)?;
    let raw = jsonrpc::parse_text(&text, SOURCE, Op::Read).map_err(|e| e.message)?;
    jsonrpc::decode(&raw, SOURCE).map_err(|e| e.message)
}

/// Резервный узел HTTP: сеть BSC и номер блока. Возвращает: описание или текст причины.
pub async fn backup_http(ctx: &Ctx) -> Result<String, String> {
    let Some(url) = backup_url(ctx, Link::Http)? else {
        return Ok("не задан (bsc.node_http_url_backup)".into());
    };
    let chain: U64 = call(ctx, &url, "eth_chainId").await?;
    same(chain.to())?;
    let block: U64 = call(ctx, &url, "eth_blockNumber").await?;
    Ok(format!("сеть {chain}, блок {block}"))
}

/// Резервный узел WebSocket: подключение и сеть BSC. Возвращает: описание или текст причины.
pub async fn backup_ws(ctx: &Ctx) -> Result<String, String> {
    let Some(url) = backup_url(ctx, Link::Ws)? else {
        return Ok("не задан (bsc.node_ws_url_backup)".into());
    };
    let conn = Conn::connect(ctx, "check-backup", &url)
        .await
        .map_err(|e| e.message)?;
    let chain: U64 = conn
        .request("eth_chainId", json!([]))
        .await
        .map_err(|e| e.message)?;
    same(chain.to())?;
    Ok(format!("сеть {chain}, подключение есть"))
}

/// Multicall3: по адресу есть код. Возвращает: описание или текст причины.
pub async fn multicall3(ctx: &Ctx) -> Result<String, String> {
    let Some(addr) = multicall::address(ctx) else {
        return Ok("не задан (dex.multicall3) — чтения пачкой JSON-RPC".into());
    };
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, "latest"]))
        .await
        .map_err(|e| e.message)?;
    if code.is_empty() {
        return Err(format!(
            "по адресу {addr} нет контракта — проверьте dex.multicall3"
        ));
    }
    Ok(format!("контракт {addr} есть"))
}

/// Обёртка нативной монеты (решение 62): по адресу dex.native_wrapped есть контракт, и он совпадает с WETH() роутеров
/// включённых DEX вида V2 (официальный источник обёртки у самой DEX; pools/mids.rs). Не задано — BNB не торгуется,
/// это не ошибка. Возвращает: описание или текст причины.
pub async fn native_wrapped(ctx: &Ctx) -> Result<String, String> {
    let Ok(addr) = ctx.addr("dex.native_wrapped") else {
        return Ok("не задана (dex.native_wrapped) — нативная монета не торгуется".into());
    };
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, "latest"]))
        .await
        .map_err(|e| e.message)?;
    if code.is_empty() {
        return Err(format!(
            "по адресу {addr} нет контракта — проверьте dex.native_wrapped"
        ));
    }
    let weth = super::pools::mids::wrapped_native(ctx).await;
    match weth.iter().find(|w| **w != addr) {
        Some(w) => Err(format!(
            "WETH() роутера V2 = {w}, а dex.native_wrapped = {addr} — проверьте адрес"
        )),
        None if weth.is_empty() => Ok(format!(
            "контракт {addr} есть (сверить с WETH() не с чем: нет включённых DEX вида V2 с dex.router)"
        )),
        None => Ok(format!(
            "контракт {addr} есть, совпадает с WETH() роутеров V2"
        )),
    }
}

/// Сверка сети с BSC mainnet. Ошибка — текст.
fn same(chain: u64) -> Result<(), String> {
    if chain == BSC_CHAIN_ID {
        Ok(())
    } else {
        Err(format!(
            "узел другой сети: chainId {chain}, у BSC mainnet {BSC_CHAIN_ID}"
        ))
    }
}
