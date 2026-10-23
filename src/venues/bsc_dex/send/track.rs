//! Файл: venues/bsc_dex/send/track.rs
//! Что делает: попадание транзакции в блок — только через свой узел (решение 42), независимо от канала: на каждый
//! новый блок (newHeads) — eth_getTransactionReceipt(хеш). Есть квитанция — «в блоке» (status 0x1) или «откат» (0x0),
//! газ = gasUsed × effectiveGasPrice. Нет квитанции дольше send.max_wait_blocks блоков — если nonce кошелька уже
//! занят другой транзакцией — «вытеснена», иначе «не попала»; повторов нет (стартовое правило Тиграна).
//! Кто вызывает: exec/trade.rs, wallet/transfer.rs.
//! От чего зависит: node/rpc.rs, node/heads.rs, send/stats.rs, num.rs, ctx.rs.
//! Настройки: send.max_wait_blocks [bsc_dex], feed.stale_after_ms [bsc_dex] (нет нового блока дольше — данные устарели).
//!
//! Источники: квитанция (status, blockNumber, gasUsed, effectiveGasPrice, logs) —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165 ;
//! null, пока нет в блоке — узел BSC internal/ethapi/api.go#L2211-L2229; схема трекера — research/flow/bsc_send.md, раздел 5.

use std::time::Duration;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, U64, U256};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

use crate::core::types::TxOutcome;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::rpc;
use super::super::num;
use super::stats::Stats;

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

/// Итог ожидания.
pub struct Tracked {
    pub outcome: TxOutcome,
    pub receipt: Option<Receipt>,
}

/// Ждёт итога транзакции. Принимает: хеш, nonce, отправителя, блок отправки, каналы (для счётчиков).
/// Ошибки: NotConfigured (send.max_wait_blocks, feed.stale_after_ms), StaleData (новых блоков нет), узел.
#[allow(clippy::too_many_arguments)]
pub async fn wait(
    ctx: &Ctx,
    heads: &Heads,
    stats: &Stats,
    hash: B256,
    nonce: u64,
    from: Address,
    sent_block: u64,
    channels: &[String],
) -> GatewayResult<Tracked> {
    let max_wait = ctx.int("send.max_wait_blocks")?;
    let stale = Duration::from_millis(ctx.int("feed.stale_after_ms")?);
    let mut rx = heads.watch();
    loop {
        if tokio::time::timeout(stale, rx.changed()).await.is_err() {
            return Err(ctx.err(ErrorCode::StaleData, format!("Нет новых блоков дольше feed.stale_after_ms — итог транзакции {hash} неизвестен")));
        }
        let Some(head) = rx.borrow_and_update().clone() else {
            continue;
        };
        let receipt: Option<Receipt> =
            rpc::call(ctx, "eth_getTransactionReceipt", json!([hash])).await?;
        if let Some(r) = receipt {
            let ok = r.status == U64::from(1u8);
            let gas = r.gas_bnb().unwrap_or_default();
            for id in channels {
                stats.update(id, |s| {
                    s.included += 1;
                    if !ok {
                        s.reverted += 1;
                        s.gas_wasted_bnb += gas;
                    }
                });
            }
            let outcome = if ok {
                TxOutcome::Included
            } else {
                TxOutcome::Reverted
            };
            return Ok(Tracked {
                outcome,
                receipt: Some(r),
            });
        }
        if head.number > sent_block + max_wait {
            let latest: U64 =
                rpc::call(ctx, "eth_getTransactionCount", json!([from, "latest"])).await?;
            if latest.to::<u64>() <= nonce {
                return Ok(Tracked {
                    outcome: TxOutcome::NotIncluded,
                    receipt: None,
                });
            }
            // Nonce занят: наша ли это транзакция, попавшая в блок между проверками, — смотрим квитанцию ещё раз.
            let again: Option<Receipt> =
                rpc::call(ctx, "eth_getTransactionReceipt", json!([hash])).await?;
            if again.is_none() {
                return Ok(Tracked {
                    outcome: TxOutcome::Replaced,
                    receipt: None,
                });
            }
        }
    }
}
