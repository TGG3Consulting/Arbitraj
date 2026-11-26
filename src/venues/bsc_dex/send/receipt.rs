//! Файл: venues/bsc_dex/send/receipt.rs
//! Что делает: квитанция транзакции (нужные поля) и её газ в BNB; поиск первой квитанции среди своих хешей
//! (исходная транзакция и её отмены).
//! Кто вызывает: send/track.rs, exec/*, wallet/transfer_status.rs (через send/track.rs — Receipt).
//! От чего зависит: node/rpc.rs, num.rs, ctx.rs.
//! Настройки: нет.
//!
//! Квитанция (status, blockNumber, gasUsed, effectiveGasPrice, logs, transactionIndex — решение 437) —
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
    #[serde(flatten)]
    pub rollup: RollupFees,
    /// Решение 437 (3): место транзакции в блоке — для разбора проигрыша по чаевым (exec/tip_judge.rs: кто изменил наш
    /// пул раньше нас). execution-apis src/schemas/receipt.yaml (transactionIndex); Option — запись без поля не ломает
    /// разбор.
    #[serde(default)]
    pub transaction_index: Option<U64>,
}

/// Дополнительные поля RPC-квитанции OP Stack. Обычная Ethereum/BSC-квитанция
/// и Nitro (L1 уже в gasUsed) обходятся без этих полей.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollupFees {
    l1_fee: Option<U256>,
    operator_fee_scalar: Option<U64>,
    operator_fee_constant: Option<U64>,
    da_footprint_gas_scalar: Option<U64>,
}

impl Receipt {
    /// Фактическая комиссия в нативной монете. Основание — receipt, включая
    /// https://specs.optimism.io/protocol/jovian/exec-engine.html#operator-fee
    pub fn gas_bnb(&self) -> Option<Decimal> {
        let used = U256::from(self.gas_used);
        let mut wei = self.effective_gas_price.checked_mul(used)?;
        if let Some(data_fee) = self.rollup.l1_fee {
            let scaled = used.checked_mul(U256::from(
                self.rollup.operator_fee_scalar.unwrap_or_default(),
            ))?;
            let operator = if self.rollup.da_footprint_gas_scalar.is_some() {
                scaled.checked_mul(U256::from(100))?
            } else {
                scaled / U256::from(1_000_000)
            };
            wei = wei
                .checked_add(data_fee)?
                .checked_add(operator)?
                .checked_add(U256::from(
                    self.rollup.operator_fee_constant.unwrap_or_default(),
                ))?;
        }
        num::to_decimal(wei, num::BNB_DECIMALS)
    }
}

/// Узлы Base должны возвращать отдельную плату за публикацию. Неполный
/// ответ оставляет собственный хеш на сверке, не подтверждая нулевую плату.
pub async fn read(ctx: &Ctx, hash: B256) -> GatewayResult<Option<Receipt>> {
    let receipt: Option<Receipt> =
        rpc::call(ctx, "eth_getTransactionReceipt", json!([hash])).await?;
    if ctx.chain_id() == Some(8453) && receipt.as_ref().is_some_and(|r| r.rollup.l1_fee.is_none()) {
        return Err(ctx.err(
            crate::gateway::errors::ErrorCode::BadResponse,
            format!("Base: квитанция {hash} не содержит l1Fee, полная комиссия неизвестна"),
        ));
    }
    Ok(receipt)
}

/// Первая найденная квитанция среди наших хешей: (индекс, квитанция). Ошибки: узел.
pub async fn find(ctx: &Ctx, ours: &[B256]) -> GatewayResult<Option<(usize, Receipt)>> {
    for (i, h) in ours.iter().enumerate() {
        let r = read(ctx, *h).await?;
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

#[cfg(test)]
mod fee_tests {
    use super::*;
    /// Поля eth_getTransactionReceipt из блоков Base 62262756 и Ethereum 26494988.
    /// События обмена в расчёте комиссии не участвуют.
    #[test]
    fn finalized_fees_match_base_and_ethereum_receipts() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/base_ethereum_fee_receipts.json"
        ))
        .unwrap();
        for (chain, wei) in [
            ("base", 394_995_829_903_i128),
            ("ethereum", 94_527_990_541_229_i128),
        ] {
            let mut row = cases[chain].clone();
            row["logs"] = serde_json::json!([]);
            let receipt: Receipt = serde_json::from_value(row).unwrap();
            assert_eq!(
                receipt.gas_bnb(),
                Some(Decimal::from_i128_with_scale(wei, 18)),
                "{chain}"
            );
        }
    }
}
