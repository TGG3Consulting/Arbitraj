//! Файл: venues/bsc_dex/quote/gas_base.rs
//! Что делает: решение 435 (задача 433-д) — цена газа сети с моделью eip1559_l1 (Arbitrum, send/gas_mode.rs):
//! базовая цена блока головы (baseFeePerGas из eth_getBlockByNumber) вместо eth_gasPrice; чаевые и запас потолка —
//! настройки площадки (send.priority_fee_gwei, send.max_fee_margin_pct). Цена в плане = база + чаевые (EIP-1559:
//! заплатит min(maxFeePerGas, база + чаевые), а потолок = база × (1 + запас) + чаевые ≥ база + чаевые).
//! Кто вызывает: quote/gas.rs (кэш на блок, котировка и отправка).
//! От чего зависит: node/rpc.rs, send/tx_1559.rs, num.rs, ctx.rs.
//! Настройки: send.priority_fee_gwei, send.max_fee_margin_pct [площадка DEX] — только при dex.gas_mode = eip1559_l1.
//!
//! Поле baseFeePerGas объекта блока — execution-apis (коммит 16caee27) src/schemas/block.yaml, Block.baseFeePerGas;
//! заголовок EIP-1559 — https://eips.ethereum.org/EIPS/eip-1559 («base_fee_per_gas»); на своём узле Arbitrum — запись
//! tests/data/arbitrum_node_2026-11-19/06_ws_eth_subscribe_newHeads.json (поле baseFeePerGas в каждом заголовке).

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::num;
use super::super::send::tx_1559::{self, Fees};

/// Из объекта блока нужна только базовая цена газа.
#[derive(Debug, Deserialize)]
pub struct BlockBase {
    /// baseFeePerGas (hex-число); у блока без EIP-1559 поля нет.
    #[serde(rename = "baseFeePerGas")]
    pub base_fee: Option<U256>,
}

/// Базовая цена газа блока. Принимает: окружение, номер блока. Возвращает: baseFeePerGas, wei.
/// Ошибки: StaleData (узел не знает блок), BadResponse (у блока нет baseFeePerGas — сеть не EIP-1559), узел.
pub async fn fetch(ctx: &Ctx, block: u64) -> GatewayResult<U256> {
    let b: Option<BlockBase> = rpc::call(
        ctx,
        "eth_getBlockByNumber",
        json!([format!("{block:#x}"), false]),
    )
    .await?;
    let b = b.ok_or_else(|| {
        ctx.err(
            ErrorCode::StaleData,
            format!("узел не отдал блок {block} — базовая цена газа неизвестна"),
        )
    })?;
    b.base_fee.ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!(
                "у блока {block} нет baseFeePerGas — узел не EIP-1559, а dex.gas_mode = eip1559_l1"
            ),
        )
    })
}

/// Чаевые (maxPriorityFeePerGas), wei, из send.priority_fee_gwei. Ошибки: NotConfigured (не задана, не в wei).
pub fn tip_wei(ctx: &Ctx) -> GatewayResult<u128> {
    let gwei = ctx.dec("send.priority_fee_gwei")?;
    num::to_units(gwei * Decimal::from(1_000_000_000u64), 0)
        .and_then(|v| u128::try_from(v).ok())
        .ok_or_else(|| {
            ctx.not_set(
                "send.priority_fee_gwei",
                format!("send.priority_fee_gwei = {gwei} не переводится в wei"),
            )
        })
}

/// Цена газа в плане: база + чаевые. Принимает: окружение, базу блока. Ошибки: как tip_wei.
pub fn planned(ctx: &Ctx, base: U256) -> GatewayResult<U256> {
    Ok(base.saturating_add(U256::from(tip_wei(ctx)?)))
}

/// Цены транзакции типа 2 от базы блока: потолок = база × (1 + send.max_fee_margin_pct/100) + чаевые.
/// Принимает: окружение, базу блока. Ошибки: NotConfigured (запас, чаевые; запас отрицательный), BadResponse (число).
pub fn fees(ctx: &Ctx, base: U256) -> GatewayResult<Fees> {
    let margin = ctx.dec("send.max_fee_margin_pct")?;
    let tip = tip_wei(ctx)?;
    tx_1559::fees(base, margin, tip).map_err(|e| ctx.err(ErrorCode::BadResponse, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Заголовки newHeads своего узла Arbitrum (06_*.json) — тот же объект блока, что у eth_getBlockByNumber без
    /// транзакций: baseFeePerGas читается (0x1318ac0, 0x1327cf0, 0x1312d00).
    #[test]
    fn reads_base_fee_from_recorded_heads() {
        let all: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../../tests/data/arbitrum_node_2026-11-19/06_ws_eth_subscribe_newHeads.json"
        ))
        .unwrap();
        let got: Vec<U256> = all
            .iter()
            .filter_map(|m| m.pointer("/params/result"))
            .map(|h| {
                serde_json::from_value::<BlockBase>(h.clone())
                    .unwrap()
                    .base_fee
                    .unwrap()
            })
            .collect();
        let want = [0x0131_8ac0u64, 0x0132_7cf0, 0x0131_2d00].map(U256::from);
        assert_eq!(got, want);
        // Блок без поля (до EIP-1559) — None, а не ошибка разбора.
        let none: BlockBase = serde_json::from_value(json!({ "number": "0x1" })).unwrap();
        assert!(none.base_fee.is_none());
    }
}
