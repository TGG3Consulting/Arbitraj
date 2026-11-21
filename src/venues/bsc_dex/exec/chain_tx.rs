//! Файл: venues/bsc_dex/exec/chain_tx.rs
//! Что делает: факт транзакции для ядра (ChainTx) из итога слежения: хеш, nonce, каналы, способ, исход, блок, газ
//! своей квитанции, чаевые (только если транзакция в блоке), отмена, занявшая nonce, — отдельно.
//! Решение 236, дополнение п. 6 (техлид 09.11 по поручению Тиграна): чаевые канала — ОТДЕЛЬНАЯ транзакция перевода
//! BNB (nonce + 1 в том же бандле, exec/txs.rs); её газ — по её квитанции (tip_gas), в ChainTx.tip_gas_bnb.
//! Кто вызывает: exec/txs.rs (chain_tx, tip_gas), exec/single_rebuild.rs, exec/swap_rebuild.rs (chain_tx),
//! resolve.rs (tip_gas — позднее выяснение).
//! От чего зависит: core/types (ChainTx), send/track.rs (Tracked), send/receipt.rs, ctx.rs (журнал).
//! Настройки: нет.
//!
//! Квитанция (gasUsed, effectiveGasPrice) — execution-apis 16caee27 src/eth/transaction.yaml#L127-L165.

use alloy_primitives::B256;
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::{ChainTx, SendMethod, TxOutcome};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::send::receipt::Receipt;
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
        approve_gas_bnb: None,
        tip_hash: None,
        tip_gas_bnb: None,
    }
}

/// Что делает: газ транзакции перевода чаевых по её квитанции — в chain.tip_gas_bnb. Нужен только когда чаевые
/// в блоке (chain.tip_bnb есть — chain_tx ставит его при «в блоке» / «откат») и хеш перевода известен (chain.tip_hash).
/// Принимает: факт транзакции, квитанцию перевода чаевых (None — не найдена). Возвращает: None — газ поставлен или не
/// нужен; Some(текст) — нужен, а квитанции нет или газ не в Decimal (для журнала: в итог сделки не вошёл). Ошибок нет.
pub fn set_tip_gas(chain: &mut ChainTx, r: Option<&Receipt>) -> Option<String> {
    let (Some(_), Some(h)) = (chain.tip_bnb, chain.tip_hash) else {
        return None;
    };
    match r.and_then(Receipt::gas_bnb) {
        Some(g) => {
            chain.tip_gas_bnb = Some(g);
            None
        }
        None => Some(format!(
            "газ перевода чаевых tx {h} (к tx {}) не прочитан: квитанции {} — в итог сделки не вошёл",
            chain.hash,
            if r.is_some() {
                "газ не в Decimal"
            } else {
                "нет"
            }
        )),
    }
}

/// Что делает: читает квитанцию перевода чаевых своим узлом (eth_getTransactionReceipt) и ставит её газ (set_tip_gas);
/// не нужен — без запроса. Квитанции нет / ошибка узла — газ не ставится, запись Warn в журнал (send.tip_gas).
/// Принимает: окружение, факт транзакции. Возвращает: тот же факт. Ошибок нет.
pub async fn tip_gas(ctx: &Ctx, mut chain: ChainTx) -> ChainTx {
    let (Some(_), Some(h)) = (chain.tip_bnb, chain.tip_hash) else {
        return chain;
    };
    let read: Result<Option<Receipt>, _> =
        rpc::call(ctx, "eth_getTransactionReceipt", json!([h])).await;
    let why = match read {
        Ok(r) => set_tip_gas(&mut chain, r.as_ref()),
        Err(e) => Some(format!(
            "газ перевода чаевых tx {h} (к tx {}) не прочитан: {} — в итог сделки не вошёл",
            chain.hash, e.message
        )),
    };
    if let Some(why) = why {
        ctx.log(Level::Warn, "send.tip_gas", why, None).await;
    }
    chain
}
