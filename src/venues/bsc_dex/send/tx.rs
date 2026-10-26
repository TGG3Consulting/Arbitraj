//! Файл: venues/bsc_dex/send/tx.rs
//! Что делает: сборка транзакции кошелька — лимит газа (eth_estimateGas + запас) и legacy-транзакция с chainId BSC;
//! общие настройки отправки, без которых транзакция не уходит (сроки ожидания, надбавка отмены).
//! Кто вызывает: exec/txs.rs, send/cancel.rs, send/track.rs.
//! От чего зависит: node/rpc.rs, node/mod.rs (chainId), ctx.rs.
//! Настройки: send.gas_limit_margin_pct, send.max_wait_blocks, send.bundle_max_blocks, send.cancel_gas_bump_pct,
//! send.replaced_confirm_blocks, send.cancel_max_gas_gwei, send.cancel_max_attempts, send.queue_max_wait_s,
//! feed.stale_after_ms [bsc_dex].
//!
//! eth_estimateGas — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/execute.yaml#L43-L60 ;
//! legacy-транзакция с chainId (EIP-155) — https://eips.ethereum.org/EIPS/eip-155 ; база газа в BSC всегда 0 —
//! research/flow/bsc_dex.md, раздел 5 (поэтому цена газа — одна gasPrice).

use std::time::Duration;

use alloy::consensus::TxLegacy;
use alloy_primitives::{Address, Bytes, TxKind, U64, U256};
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::BSC_CHAIN_ID;
use super::super::node::rpc;
use super::super::num;

/// Настройки ожидания и отмены — читаются до отправки: не задано — транзакция не уходит.
#[derive(Debug, Clone, Copy)]
pub struct SendCfg {
    /// send.max_wait_blocks — сколько блоков ждать квитанции до отмены.
    pub max_wait: u64,
    /// send.bundle_max_blocks — maxBlockNumber бандла = блок + это число.
    pub bundle_max: u64,
    /// send.cancel_gas_bump_pct — на сколько % цена газа отмены выше прошлой.
    pub bump_pct: Decimal,
    /// feed.stale_after_ms — нет нового блока дольше: предупреждение в журнал (очередь кошелька ждёт дальше).
    pub stale: Duration,
    /// send.replaced_confirm_blocks — nonce занят без своей квитанции: сколько ещё блоков искать её.
    pub replaced_confirm: u64,
    /// send.cancel_max_gas_gwei в wei — потолок цены газа отмены.
    pub cancel_max_wei: U256,
    /// send.cancel_max_attempts — сколько отмен одной транзакции.
    pub cancel_max_attempts: u64,
    /// send.queue_max_wait_s — сколько ждать итога одной транзакции до остановки очереди.
    pub queue_max_wait: Duration,
}

impl SendCfg {
    /// Читает настройки. Ошибки: NotConfigured — не задано или send.max_wait_blocks < send.bundle_max_blocks
    /// (бандл мог бы попасть в блок, когда трекер уже отменяет).
    pub fn read(ctx: &Ctx) -> GatewayResult<Self> {
        let cfg = Self {
            max_wait: ctx.int("send.max_wait_blocks")?,
            bundle_max: ctx.int("send.bundle_max_blocks")?,
            bump_pct: ctx.dec("send.cancel_gas_bump_pct")?,
            stale: Duration::from_millis(ctx.int("feed.stale_after_ms")?),
            replaced_confirm: ctx.int("send.replaced_confirm_blocks")?,
            cancel_max_wei: num::to_units(
                ctx.dec("send.cancel_max_gas_gwei")? * Decimal::from(1_000_000_000u64),
                0,
            )
            .ok_or_else(|| {
                ctx.not_set(
                    "send.cancel_max_gas_gwei",
                    "send.cancel_max_gas_gwei не переводится в wei",
                )
            })?,
            cancel_max_attempts: ctx.int("send.cancel_max_attempts")?,
            queue_max_wait: Duration::from_secs(ctx.int("send.queue_max_wait_s")?),
        };
        if cfg.max_wait < cfg.bundle_max {
            return Err(ctx.not_set(
                "send.max_wait_blocks",
                format!(
                    "send.max_wait_blocks ({}) меньше send.bundle_max_blocks ({}): бандл может попасть в блок, когда трекер уже отменяет транзакцию — отправка не работает",
                    cfg.max_wait, cfg.bundle_max
                ),
            ));
        }
        Ok(cfg)
    }
}

/// Лимит газа: eth_estimateGas × (1 + send.gas_limit_margin_pct/100).
/// Принимает: отправителя, адрес, данные, сумму BNB (wei). Ошибки: откат (текст узла), настройка, BadResponse.
pub async fn limit(
    ctx: &Ctx,
    from: Address,
    to: Address,
    data: &Bytes,
    value: U256,
) -> GatewayResult<u64> {
    let est: U64 = rpc::call(
        ctx,
        "eth_estimateGas",
        json!([{ "from": from, "to": to, "input": data, "value": value }]),
    )
    .await?;
    let margin = ctx.dec("send.gas_limit_margin_pct")?;
    let g = Decimal::from(est.to::<u64>()) * (Decimal::ONE + margin / Decimal::ONE_HUNDRED);
    u64::try_from(g.ceil()).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Лимит газа {g} не помещается"),
        )
    })
}

/// Legacy-транзакция с chainId BSC (EIP-155). Ошибки: BadResponse — цена газа больше u128.
pub fn legacy(
    ctx: &Ctx,
    nonce: u64,
    price: U256,
    gas: u64,
    to: Address,
    value: U256,
    input: Bytes,
) -> GatewayResult<TxLegacy> {
    let gas_price = u128::try_from(price).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Цена газа {price} wei не помещается в транзакцию"),
        )
    })?;
    Ok(TxLegacy {
        chain_id: Some(BSC_CHAIN_ID),
        nonce,
        gas_price,
        gas_limit: gas,
        to: TxKind::Call(to),
        value,
        input,
    })
}
