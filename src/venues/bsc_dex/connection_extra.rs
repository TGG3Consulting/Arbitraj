//! Файл: venues/bsc_dex/connection_extra.rs
//! Что делает: части «Проверить подключение» DEX для резервного узла и Multicall3: резервный узел HTTP (сеть, блок)
//! и WebSocket (сеть по eth_chainId на соединении) — по своим адресам, мимо переключения (node/failover.rs);
//! Multicall3 — есть ли код по адресу dex.multicall3 (eth_getCode) и отвечает ли он на eth_call (getBlockNumber(),
//! решение 121, A3). Не задано — «не задан», это не ошибка.
//! Кто вызывает: connection.rs.
//! От чего зависит: ctx.rs, node/rpc.rs (post), node/ws.rs (Conn::connect), node/jsonrpc.rs, node/multicall.rs, node/call.rs.
//! Обёртка нативной монеты (решение 62) — код по адресу dex.native_wrapped и совпадение с WETH() роутеров V2.
//! Настройки: dex.evm_chain_id (сверка сети, решение 416; node/chain_id.rs), bsc.node_http_url_backup, bsc.node_ws_url_backup, dex.multicall3, dex.native_wrapped; секреты bsc.node_*_key_backup.
//!
//! eth_chainId, eth_blockNumber, eth_getCode — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth
//! Multicall3.getBlockNumber() — https://github.com/mds1/multicall3/blob/main/src/Multicall3.sol#L170 .

use alloy_primitives::{Bytes, U64};
use serde_json::json;

use super::ctx::Ctx;
use super::errors::Op;
use super::node::chain_id::same_chain;
use super::node::failover::{Link, Node};
use super::node::rpc::{self, SOURCE};
use super::node::ws::Conn;
use super::node::{jsonrpc, multicall};
use super::pools::{mids, wrapped};

/// Адрес резервного узла с ключом. None — резервный не задан. Ошибка — текст (ключ не введён и т.п.).
fn backup_url(ctx: &Ctx, link: Link) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    let (url, key) = link.names(Node::Backup);
    if ctx.settings.text(url, ctx.at()).is_err() {
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

/// Резервный узел HTTP: сеть (dex.evm_chain_id, решение 416) и номер блока. Возвращает: описание или текст причины.
pub async fn backup_http(ctx: &Ctx) -> Result<String, String> {
    let Some(url) = backup_url(ctx, Link::Http)? else {
        return Ok("не задан (bsc.node_http_url_backup)".into());
    };
    let chain: U64 = call(ctx, &url, "eth_chainId").await?;
    // Решение 416 (ревью М-1): ответ резервного узла — в подтверждения сети (сверка перед подписью, node/chain_seen.rs).
    if let (Ok(plain), Ok(want)) = (
        ctx.settings
            .text(Link::Http.names(Node::Backup).0, ctx.at()),
        super::node::chain_id::chain_id(ctx),
    ) {
        ctx.chains.note(&plain, chain.to(), want);
    }
    same_chain(ctx, chain.to())?;
    let block: U64 = call(ctx, &url, "eth_blockNumber").await?;
    Ok(format!("сеть {chain}, блок {block}"))
}

/// Резервный узел WebSocket: подключение и сеть (dex.evm_chain_id, решение 416). Возвращает: описание или текст причины.
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
    same_chain(ctx, chain.to())?;
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
    // Решение 121 (A3): контракт не только есть, но и отвечает на eth_call (getBlockNumber() — номер блока узла).
    let call = multicall::IMulticall3::getBlockNumberCall {};
    let n = super::node::call::view(ctx, addr, &call, "latest")
        .await
        .map_err(|e| {
            format!(
                "контракт {addr} есть, но eth_call getBlockNumber() не прошёл: {}",
                e.message
            )
        })?;
    Ok(format!("контракт {addr} есть, отвечает: блок {n}"))
}

/// Обёртка нативной монеты (решение 62): по адресу dex.native_wrapped есть контракт, и он совпадает с WETH() роутеров
/// включённых DEX вида V2; чужой WETH() — факт, если хоть один роутер вернул её, иначе ошибка (решение 270). Не задано — BNB не торгуется,
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
    // Решение 270: чужой WETH() (другая обёртка, fstswap) — факт, не ошибка, если хоть один роутер вернул
    // dex.native_wrapped; ни один не вернул — ошибка, как раньше (pools/wrapped.rs::verdict).
    wrapped::verdict(addr, mids::router_weths(ctx).await)
}
