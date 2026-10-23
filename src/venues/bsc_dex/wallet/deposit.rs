//! Файл: venues/bsc_dex/wallet/deposit.rs
//! Что делает: функции контракта вокруг кошелька, которым нужно общее состояние адаптера: статус зачисления
//! (wallet/transfer_status.rs) со снятием ожидаемого поступления при любом завершении («зачислен», «отклонён»,
//! хеш или квитанция не разбираются — wallet/inflows.rs), отказ от депозита (вывод не состоялся), состояние очереди
//! кошелька и «Снять очередь после ручной проверки» (wallet/nonce.rs; журнал, событие ядру → Telegram).
//! Кто вызывает: mod.rs (deposit_status, deposit_abandoned, wallet_queue, release_wallet_queue).
//! От чего зависит: parts.rs, wallet/transfer_status.rs, wallet/inflows.rs, wallet/nonce.rs, send/stats.rs.
//! Настройки: через вызываемые — send.deposit_confirmations, venue.bsc_network, transfer.max_time_s.

use crate::core::types::{Asset, Transfer, TransferStatus, WalletQueue};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

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

/// Состояние очереди кошелька. Ошибок нет.
pub fn queue(p: &Parts) -> WalletQueue {
    WalletQueue {
        venue: p.ctx.venue(),
        state: p.nonces.state(),
    }
}

/// «Снять очередь после ручной проверки»: журнал, событие ядру (Telegram send.queue_released).
/// Принимает: кто снимает. Ошибки: NotApplicable — очередь не остановлена.
pub async fn release(p: &Parts, by: &str) -> GatewayResult<WalletQueue> {
    let ctx = &p.ctx;
    let state = p
        .nonces
        .release()
        .map_err(|why| ctx.err(ErrorCode::NotApplicable, why))?;
    let msg = format!("Очередь кошелька снята вручную после проверки ({by})");
    ctx.log(Level::Warn, "send.queue_released", msg, None).await;
    let q = WalletQueue {
        venue: ctx.venue(),
        state,
    };
    p.stats.emit(MarketEvent::WalletQueue(q.clone()));
    Ok(q)
}
