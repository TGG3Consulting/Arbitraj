//! Файл: venues/bsc_dex/send/receipt.rs
//! Что делает: квитанция транзакции (нужные поля) и её газ в BNB; поиск первой квитанции среди своих хешей
//! (исходная транзакция и её отмены).
//! Кто вызывает: send/track.rs, exec/*, wallet/transfer_status.rs (через send/track.rs — Receipt).
//! От чего зависит: node/rpc.rs, num.rs, ctx.rs.
//! Настройки: нет.
//!
//! Квитанция (status, blockNumber, gasUsed, effectiveGasPrice, logs) —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165 ;
//! null, пока нет в блоке — узел BSC internal/ethapi/api.go#L2211-L2229.

use alloy::rpc::types::Log;
use alloy_primitives::{B256, U64, U256};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::num;

/// Квитанция: только нужные поля.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub status: U64,
    pub block_number: U64,
    pub gas_used: U64,
    pub effective_gas_price: U256,
    pub logs: Vec<Log>,
}

impl Receipt {
    /// Газ транзакции в BNB. None — не помещается в Decimal.
    pub fn gas_bnb(&self) -> Option<Decimal> {
        num::to_decimal(
            self.effective_gas_price
                .saturating_mul(U256::from(self.gas_used)),
            num::BNB_DECIMALS,
        )
    }
}

/// Первая найденная квитанция среди наших хешей: (индекс, квитанция). Ошибки: узел.
pub async fn find(ctx: &Ctx, ours: &[B256]) -> GatewayResult<Option<(usize, Receipt)>> {
    for (i, h) in ours.iter().enumerate() {
        let r: Option<Receipt> = rpc::call(ctx, "eth_getTransactionReceipt", json!([h])).await?;
        if let Some(r) = r {
            return Ok(Some((i, r)));
        }
    }
    Ok(None)
}

/// Есть ли среди наших хешей транзакция, которую узел знает уже в блоке (eth_getTransactionByHash: blockNumber не null)
/// — квитанция может появиться позже (узел ещё индексирует, балансировщик RPC). Ошибки: узел.
/// eth_getTransactionByHash — execution-apis 16caee27 src/eth/transaction.yaml (null — не найдена).
pub async fn in_block(ctx: &Ctx, ours: &[B256]) -> GatewayResult<bool> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Tx {
        block_number: Option<U64>,
    }
    for h in ours {
        let t: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([h])).await?;
        if t.and_then(|t| t.block_number).is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}
