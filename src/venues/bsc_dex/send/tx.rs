//! Файл: venues/bsc_dex/send/tx.rs
//! Что делает: сборка транзакции кошелька — лимит газа (eth_estimateGas + запас) и legacy-транзакция с chainId сети
//! площадки (решение 416: dex.evm_chain_id, читает вызывающий до отправки — node/chain_id.rs);
//! общие настройки отправки, без которых транзакция не уходит (сроки ожидания, надбавка отмены, запас газа отмены).
//! Решение 398: всё, что трекер и отмена используют ПОСЛЕ отправки, читается в SendCfg до неё и передаётся значением —
//! после отправки настройки не читаются (сторож tests/settings_before_send_guard.rs).
//! Кто вызывает: exec/txs.rs, send/cancel.rs, send/track.rs.
//! От чего зависит: node/rpc.rs, ctx.rs, send/drop_wait.rs.
//! Настройки: send.gas_limit_margin_pct, send.max_wait_blocks, send.bundle_max_blocks, send.cancel_gas_bump_pct,
//! send.replaced_confirm_blocks, send.cancel_max_gas_gwei, send.cancel_max_attempts, send.queue_max_wait_s,
//! feed.stale_after_ms [bsc_dex]; решение 437 — send.drop_wait_ms и (ревью 439) feed.max_block_age_ms — только
//! dex.gas_mode = eip1559_l1, читает send/drop_wait.rs.
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
use super::super::node::rpc;
use super::super::num;
use super::drop_wait::{self, DropWait};

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
    /// send.gas_limit_margin_pct — запас лимита газа отмены (решение 398: отмена уходит после исходной, значение — до неё).
    pub gas_margin_pct: Decimal,
    /// Решение 437: только dex.gas_mode = eip1559_l1 (Arbitrum) — send.drop_wait_ms: сколько ждать квитанции после
    /// отправки, прежде чем по nonce («latest») решить «выброшена» (отмен у этой сети нет, send/track_l1.rs); ревью
    /// 439 (1) — с порогом возраста блока feed.max_block_age_ms (send/drop_wait.rs).
    /// None — legacy (BSC): трекер с отменами, как раньше.
    pub drop_wait: Option<DropWait>,
}

impl SendCfg {
    /// Читает настройки. Ошибки: NotConfigured — не задано (у eip1559_l1 — и send.drop_wait_ms,
    /// feed.max_block_age_ms), send.max_wait_blocks < send.bundle_max_blocks (бандл мог бы попасть в блок, когда трекер
    /// уже отменяет) или проверки окна eip1559_l1 (send/drop_wait.rs read: ревью 437, п. 7; ревью 439 (1)).
    pub fn read(ctx: &Ctx) -> GatewayResult<Self> {
        let mut cfg = Self {
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
            gas_margin_pct: ctx.dec("send.gas_limit_margin_pct")?,
            // Читается ниже, после проверки блоков (send/drop_wait.rs).
            drop_wait: None,
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
        // Решение 437: окно eip1559_l1 и его проверки (ревью 437, п. 7; ревью 439 (1)) — send/drop_wait.rs.
        cfg.drop_wait = drop_wait::read(ctx, cfg.queue_max_wait)?;
        Ok(cfg)
    }
}

/// Лимит газа: eth_estimateGas × (1 + send.gas_limit_margin_pct/100).
/// Принимает: отправителя, адрес (или создание контракта — TxKind::Create, без поля to), данные, сумму BNB (wei).
/// Ошибки: откат (текст узла), настройка, BadResponse.
pub async fn limit(
    ctx: &Ctx,
    from: Address,
    to: impl Into<TxKind>,
    data: &Bytes,
    value: U256,
) -> GatewayResult<u64> {
    let est = estimate(ctx, from, to, data, value).await?;
    with_margin(ctx, est, ctx.dec("send.gas_limit_margin_pct")?)
}

/// Лимит газа с запасом, прочитанным заранее (решение 398: отмена после отправки исходной — SendCfg::gas_margin_pct).
/// Принимает: окружение, запас (%), отправителя, адрес, данные, сумму BNB (wei). Ошибки: откат (текст узла), BadResponse.
pub async fn limit_by(
    ctx: &Ctx,
    margin: Decimal,
    from: Address,
    to: impl Into<TxKind>,
    data: &Bytes,
    value: U256,
) -> GatewayResult<u64> {
    let est = estimate(ctx, from, to, data, value).await?;
    with_margin(ctx, est, margin)
}

/// eth_estimateGas вызова. Принимает: отправителя, адрес (или TxKind::Create), данные, сумму. Ошибки: откат, узел.
async fn estimate(
    ctx: &Ctx,
    from: Address,
    to: impl Into<TxKind>,
    data: &Bytes,
    value: U256,
) -> GatewayResult<U64> {
    let body = match to.into() {
        TxKind::Call(to) => json!({ "from": from, "to": to, "input": data, "value": value }),
        TxKind::Create => json!({ "from": from, "input": data, "value": value }),
    };
    rpc::call(ctx, "eth_estimateGas", json!([body])).await
}

/// Оценка газа × (1 + запас/100), вверх до целого. Ошибки: BadResponse — не помещается в u64.
fn with_margin(ctx: &Ctx, est: U64, margin: Decimal) -> GatewayResult<u64> {
    let g = Decimal::from(est.to::<u64>()) * (Decimal::ONE + margin / Decimal::ONE_HUNDRED);
    u64::try_from(g.ceil()).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Лимит газа {g} не помещается"),
        )
    })
}

/// Legacy-транзакция с chainId сети (EIP-155); chain — dex.evm_chain_id, прочитанный вызывающим до отправки (решение 416;
/// отмена — из Watch, решение 398); to — адрес вызова или TxKind::Create (развёртывание исполнителя, решение 122).
/// Ошибки: BadResponse — цена газа больше u128.
#[allow(clippy::too_many_arguments)]
pub fn legacy(
    ctx: &Ctx,
    chain: u64,
    nonce: u64,
    price: U256,
    gas: u64,
    to: impl Into<TxKind>,
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
        chain_id: Some(chain),
        nonce,
        gas_price,
        gas_limit: gas,
        to: to.into(),
        value,
        input,
    })
}
