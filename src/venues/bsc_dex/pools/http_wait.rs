//! Файл: venues/bsc_dex/pools/http_wait.rs
//! Что делает: ожидание, пока узел HTTP дойдёт до блока узла WebSocket (начало сессии слушателя пулов): состояние
//! читается по HTTP на блоке B узла WS — HTTP должен его знать. Отставание провайдера — не сбой, пока HTTP продвигается.
//! Номер блока узла HTTP — из опроса node/heads.rs (решение 57).
//!
//! Кто вызывает: pools/admit.rs.
//! От чего зависит: node/heads.rs, pools/listen.rs (Pools).
//! Настройки: feed.stale_after_ms [bsc_dex].
//!
//! eth_blockNumber — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth .

use std::time::Duration;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::listen::Pools;

/// Ждёт, пока узел HTTP дойдёт до блока узла WS. Номер блока узла HTTP — из опроса блоков (node/heads.rs, решение 57):
/// отдельных eth_blockNumber здесь нет. Сколько угодно блоков, пока номер растёт (отставание провайдера — не сбой).
/// Ошибки: StaleData — номер узла HTTP не рос дольше feed.stale_after_ms; NotConfigured; Internal (опрос остановлен).
pub async fn http_reached(p: &Pools, block: u64) -> GatewayResult<()> {
    let wait = Duration::from_millis(p.ctx.int("feed.stale_after_ms")?);
    let mut rx = p.heads.watch();
    loop {
        let http = rx.borrow_and_update().as_ref().map(|h| h.number);
        if http.is_some_and(|n| n >= block) {
            return Ok(());
        }
        match tokio::time::timeout(wait, rx.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(p.ctx.err(ErrorCode::Internal, "опрос блоков остановлен"));
            }
            Err(_) => {
                let at = http.map_or("—".to_string(), |n| n.to_string());
                return Err(p.ctx.err(
                    ErrorCode::StaleData,
                    format!("узел HTTP стоит на блоке {at} дольше feed.stale_after_ms, узел WebSocket на {block}"),
                ));
            }
        }
    }
}
