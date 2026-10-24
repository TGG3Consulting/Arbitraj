//! Файл: venues/bsc_dex/exec/chain_tx.rs
//! Что делает: факт транзакции для ядра (ChainTx) из итога слежения: хеш, nonce, каналы, способ, исход, блок, газ
//! своей квитанции, чаевые (только если транзакция в блоке), отмена, занявшая nonce, — отдельно.
//! Кто вызывает: exec/txs.rs.
//! От чего зависит: core/types (ChainTx), send/track.rs (Tracked).
//! Настройки: нет.

use alloy_primitives::B256;
use rust_decimal::Decimal;

use crate::core::types::{ChainTx, SendMethod, TxOutcome};

use super::super::send::track::Tracked;

/// ChainTx из итога. Газ — только самой транзакции (её квитанция); отмена, занявшая nonce, — отдельно
/// (cancel_hash, cancel_gas_bnb). Ошибок нет.
pub fn chain_tx(
    hash: B256,
    nonce: u64,
    channels: Vec<String>,
    method: SendMethod,
    t: &Tracked,
    tip_bnb: Option<Decimal>,
) -> ChainTx {
    let own = t.receipt.as_ref();
    ChainTx {
        hash,
        nonce,
        channels,
        method,
        outcome: t.outcome,
        block: own.map(|r| r.block_number.to()),
        gas_used: own.map(|r| r.gas_used.to()),
        gas_bnb: own.and_then(|r| r.gas_bnb()),
        tip_bnb: if t.outcome == TxOutcome::Included || t.outcome == TxOutcome::Reverted {
            tip_bnb
        } else {
            None
        },
        cancel_hash: t.cancel.as_ref().map(|c| c.0),
        cancel_gas_bnb: t.cancel.as_ref().and_then(|c| c.1.gas_bnb()),
    }
}
