//! Файл: venues/bsc_dex/wallet/deposit.rs
//! Что делает: функции контракта вокруг кошелька, которым нужно общее состояние адаптера: статус зачисления
//! (wallet/transfer_status.rs) со снятием ожидаемого поступления при любом завершении («зачислен», «отклонён»,
//! хеш или квитанция не разбираются — wallet/inflows.rs), отказ от депозита (вывод не состоялся), состояние очереди
//! кошелька и «Снять очередь после ручной проверки» (wallet/nonce.rs; журнал, событие ядру → Telegram).
//! Кто вызывает: mod.rs (deposit_status, deposit_abandoned, wallet_queue, release_wallet_queue).
//! От чего зависит: parts.rs, wallet/transfer_status.rs, wallet/inflows.rs, wallet/nonce.rs, send/stats.rs.
//! Настройки: через вызываемые — send.deposit_confirmations, venue.bsc_network, transfer.max_time_s.

use alloy_primitives::U64;
use serde_json::json;

use crate::core::types::{Asset, Transfer, TransferStatus, WalletQueue, WalletQueueState};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::node::rpc;
use super::super::parts::Parts;
use super::{signer, transfer_status};

/// Статус зачисления по хешу; завершение снимает ожидание поступления (один раз на хеш).
/// Ошибки: узел, NotConfigured, BadResponse.
pub async fn status(p: &Parts, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
    let ctx = &p.ctx;
    let w = signer::address(ctx)?;
    let res = transfer_status::deposit_status(ctx, &p.tokens, asset, tx_hash, w).await;
    let finished = match &res {
        Ok(t) => matches!(
            t.status,
            TransferStatus::Credited | TransferStatus::Rejected
        ),
        Err(e) => e.code == ErrorCode::BadResponse,
    };
    if finished && let Some(token) = asset.bsc_contract {
        p.inflows.done(token, tx_hash);
    }
    res
}

/// Вывод на кошелёк не состоялся: снимает ожидание поступления, запись в журнал. Ошибок нет.
pub async fn abandoned(p: &Parts, asset: &Asset) {
    if let Some(token) = asset.bsc_contract {
        p.inflows.abandoned(token);
        let msg = format!(
            "{}: вывод на кошелёк не состоялся — ожидание поступления снято",
            asset.ticker
        );
        p.ctx.log(Level::Info, "dex.inflow", msg, None).await;
    }
}

/// Состояние очереди кошелька (после запуска — из SQLite); у остановленной — занят ли её nonce в блоке. Ошибок нет.
pub async fn queue(p: &Parts) -> WalletQueue {
    p.nonces.restore(&p.ctx).await;
    let state = p.nonces.state();
    let nonce_taken = taken(p, &state).await.ok().flatten();
    WalletQueue {
        venue: p.ctx.venue(),
        state,
        nonce_taken,
    }
}

/// Занят ли nonce остановленной транзакции: eth_getTransactionCount(кошелёк, "latest") > nonce (execution-apis
/// 16caee27 src/eth/state.yaml). None — очередь не остановлена или nonce неизвестен. Ошибки: ключ, узел.
async fn taken(p: &Parts, state: &WalletQueueState) -> GatewayResult<Option<bool>> {
    let WalletQueueState::Halted { nonce: Some(n), .. } = state else {
        return Ok(None);
    };
    let w = signer::address(&p.ctx)?;
    let c: U64 = rpc::call(&p.ctx, "eth_getTransactionCount", json!([w, "latest"])).await?;
    Ok(Some(c.to::<u64>() > *n))
}

/// «Снять очередь после ручной проверки»: перед снятием — nonce кошелька на «latest»; nonce ещё не занят (висящая
/// транзакция может пройти позже) — только с подтверждением pending_ok. Журнал, SQLite, событие ядру.
/// Принимает: кто снимает, подтверждение. Ошибки: NotApplicable — не остановлена; NonceConflict — nonce не занят,
/// подтверждения нет; узел — nonce не прочитан (без него снимать нельзя).
pub async fn release(p: &Parts, by: &str, pending_ok: bool) -> GatewayResult<WalletQueue> {
    let ctx = &p.ctx;
    p.nonces.restore(ctx).await;
    let before = p.nonces.state();
    let nonce_taken = taken(p, &before).await?;
    if nonce_taken == Some(false) && !pending_ok {
        let n = match &before {
            WalletQueueState::Halted { nonce: Some(n), .. } => *n,
            _ => 0,
        };
        return Err(ctx.err(
            ErrorCode::NonceConflict,
            format!("nonce {n} ещё не занят — висящая транзакция может пройти позже; снимать только с подтверждением"),
        ));
    }
    let state = p
        .nonces
        .release()
        .map_err(|why| ctx.err(ErrorCode::NotApplicable, why))?;
    let note = if nonce_taken == Some(false) {
        ", nonce ещё не был занят — подтверждено"
    } else {
        ""
    };
    let msg = format!("Очередь кошелька снята вручную после проверки ({by}{note})");
    ctx.log(Level::Warn, "send.queue_released", msg, None).await;
    super::queue_store::save(ctx, &state).await;
    let q = WalletQueue {
        venue: ctx.venue(),
        state,
        nonce_taken: None,
    };
    p.stats.emit(MarketEvent::WalletQueue(q.clone()));
    Ok(q)
}
