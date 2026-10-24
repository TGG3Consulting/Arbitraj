//! Файл: venues/bsc_dex/connection.rs
//! Что делает: «Проверить подключение» площадки DEX — узел по HTTP (сеть, номер блока), узел по WebSocket
//! (сеть, подписка newHeads получает блок), поддержка eth_simulateV1 (без неё налоги монет не проверить — торговли
//! нет, ответ техлида в Issue #7), кошелёк (ключ → адрес, баланс BNB). Ответ — «Подключено»
//! или точная причина по каждой части.
//! Кто вызывает: mod.rs (контракт: check_connection) ← api (кнопка на экране «Площадки»).
//! От чего зависит: ctx.rs, node/rpc.rs, node/ws.rs, wallet/signer.rs, wallet/balances.rs.
//! Настройки: bsc.node_http_url, bsc.node_ws_url, http.timeout_ms, venue.ws_silence_ms [bsc_dex],
//! feed.stale_after_ms [bsc_dex] (сколько ждать блок по подписке); секреты bsc.node_*_key, bsc_dex.wallet_key.
//!
//! Методы: eth_chainId, eth_blockNumber, eth_subscribe newHeads, eth_simulateV1 —
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth ;
//! так проверку предлагает research/flow/bsc_dex.md, схема адаптера п. 9.

use std::time::Duration;

use alloy_primitives::U64;
use serde_json::json;

use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::ctx::Ctx;
use super::node::ws::Conn;
use super::node::{BSC_CHAIN_ID, rpc};
use super::wallet::{balances, signer};

/// Полная проверка. Возвращает: ConnectionCheck (ok — только если все части в порядке). Ошибок нет.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    let parts = [
        ("Узел HTTP", http(ctx).await),
        ("Узел WebSocket", ws(ctx).await),
        ("Симуляция", simulate(ctx).await),
        ("Кошелёк", wallet(ctx).await),
    ];
    let ok = parts.iter().all(|(_, r)| r.is_ok());
    let details = parts
        .iter()
        .map(|(name, r)| match r {
            Ok(s) => format!("{name}: {s}"),
            Err(e) => format!("{name}: ОШИБКА — {e}"),
        })
        .collect::<Vec<_>>()
        .join("; ");
    ConnectionCheck {
        ok,
        details: if ok {
            format!("Подключено. {details}")
        } else {
            details
        },
    }
}

/// Узел по HTTP: сеть BSC и номер блока. Возвращает: описание или текст причины.
async fn http(ctx: &Ctx) -> Result<String, String> {
    let chain: U64 = rpc::call(ctx, "eth_chainId", json!([]))
        .await
        .map_err(|e| e.message)?;
    same_chain(chain.to())?;
    let block: U64 = rpc::call(ctx, "eth_blockNumber", json!([]))
        .await
        .map_err(|e| e.message)?;
    Ok(format!("сеть {chain}, блок {block}"))
}

/// Узел по WebSocket: сеть BSC и блок по подписке newHeads за feed.stale_after_ms.
async fn ws(ctx: &Ctx) -> Result<String, String> {
    let run = async {
        let conn = Conn::open(ctx, "check").await?;
        let chain: U64 = conn.request("eth_chainId", json!([])).await?;
        same_chain(chain.to()).map_err(|m| ctx.err(ErrorCode::BadResponse, m))?;
        let mut sub = conn.subscribe(json!(["newHeads"])).await?;
        let wait = Duration::from_millis(ctx.int("feed.stale_after_ms")?);
        match tokio::time::timeout(wait, sub.rx.recv()).await {
            Ok(Some(_)) => Ok::<_, crate::gateway::errors::GatewayError>(format!(
                "сеть {chain}, подписка newHeads получает блоки"
            )),
            Ok(None) => Err(ctx.err(
                ErrorCode::Unreachable,
                conn.end_reason().unwrap_or_default(),
            )),
            Err(_) => Err(ctx.err(
                ErrorCode::StaleData,
                format!(
                    "подписка newHeads не прислала блок за {} мс (feed.stale_after_ms)",
                    wait.as_millis()
                ),
            )),
        }
    };
    run.await.map_err(|e| e.message)
}

/// Поддержка eth_simulateV1: пустой блок вызовов на «latest» (execution-apis src/eth/execute.yaml, eth_simulateV1).
/// Любая ошибка узла — проверка налогов невозможна. Возвращает: описание или текст причины.
async fn simulate(ctx: &Ctx) -> Result<String, String> {
    let payload = json!({ "blockStateCalls": [{ "calls": [] }], "validation": false });
    let r: GatewayResult<serde_json::Value> =
        rpc::call(ctx, "eth_simulateV1", json!([payload, "latest"])).await;
    match r {
        Ok(_) => Ok("узел поддерживает eth_simulateV1".into()),
        Err(e) => Err(format!(
            "узел не поддерживает eth_simulateV1 — проверка налогов невозможна, торговли нет ({})",
            e.message
        )),
    }
}

/// Кошелёк: ключ из секретов → адрес, баланс BNB.
async fn wallet(ctx: &Ctx) -> Result<String, String> {
    let run = async {
        let addr = signer::address(ctx)?;
        let bnb = balances::bnb(ctx, addr).await?;
        GatewayResult::Ok(format!("адрес {addr}, BNB {bnb}"))
    };
    run.await.map_err(|e| e.message)
}

/// Сверка сети с BSC mainnet. Ошибка — текст причины.
fn same_chain(chain: u64) -> Result<(), String> {
    if chain == BSC_CHAIN_ID {
        Ok(())
    } else {
        Err(format!(
            "узел другой сети: chainId {chain}, у BSC mainnet {BSC_CHAIN_ID}"
        ))
    }
}
