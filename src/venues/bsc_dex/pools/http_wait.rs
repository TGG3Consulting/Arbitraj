//! Файл: venues/bsc_dex/pools/http_wait.rs
//! Что делает: ожидание, пока узел HTTP дойдёт до блока узла WebSocket (начало сессии слушателя пулов): состояние
//! читается по HTTP на блоке B узла WS — HTTP должен его знать. Отставание провайдера — не сбой, пока HTTP продвигается.
//!
//! Кто вызывает: pools/session.rs.
//! От чего зависит: node/rpc.rs, node/heads.rs, pools/listen.rs (Pools).
//! Настройки: feed.stale_after_ms [bsc_dex].
//!
//! eth_blockNumber — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth .

use std::time::Duration;

use alloy_primitives::U64;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::node::rpc;
use super::listen::Pools;

/// Ждёт, пока узел HTTP дойдёт до блока узла WS: проверка на каждый новый блок newHeads, сколько угодно блоков, пока
/// узел HTTP продвигается (отставание провайдера — не сбой). Ошибки: StaleData — блок узла HTTP не менялся дольше
/// feed.stale_after_ms или новых блоков нет; узел; NotConfigured.
pub async fn http_reached(p: &Pools, block: u64) -> GatewayResult<()> {
    let wait = Duration::from_millis(p.ctx.int("feed.stale_after_ms")?);
    let mut rx = p.heads.watch();
    let (mut last, mut moved) = (0u64, tokio::time::Instant::now());
    loop {
        let http: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
        let http = http.to::<u64>();
        if http >= block {
            return Ok(());
        }
        if http > last {
            (last, moved) = (http, tokio::time::Instant::now());
        } else if moved.elapsed() > wait {
            return Err(p.ctx.err(
                ErrorCode::StaleData,
                format!("узел HTTP стоит на блоке {http} дольше feed.stale_after_ms, узел WebSocket на {block}"),
            ));
        }
        match tokio::time::timeout(wait, rx.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(p
                    .ctx
                    .err(ErrorCode::Internal, "поток новых блоков остановлен"));
            }
            Err(_) => {
                return Err(p.ctx.err(
                    ErrorCode::StaleData,
                    "нет новых блоков дольше feed.stale_after_ms — узел HTTP не с чем сравнить",
                ));
            }
        }
    }
}
