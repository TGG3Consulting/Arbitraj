//! Файл: venues/bitget/deposits.rs
//! Что делает: статус депозита Bitget по хешу транзакции: записи депозитов монеты в окне venue.history_window_s
//! (records.rs), сравнение tradeId с хешем (gateway::tx_hash::same).
//! Кто вызывает: mod.rs (deposit_status).
//! От чего зависит: records.rs, num.rs, ctx.rs, gateway/tx_hash.rs.
//! Настройки: через records.rs — venue.history_window_s [bitget].
//!
//! Официально — Get Deposit Records GET /api/v2/spot/wallet/deposit-records
//! https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account#get-deposit-records: поиска по хешу нет
//! (параметры coin, orderId, startTime*, endTime*, idLessThan, limit ≤ 100) — ищется в первой странице окна (100
//! последних записей монеты). status: «pending: pending confirmation», «fail», «success» (в таблице поле названо
//! «Withdrawal status» — небрежность документации). Статуса «предварительно зачислено» (как MEXC PRE_SUCCESS, решение
//! 150) в документации нет — удержания вывода (withdraw_hold) у Bitget нет, пока живой депозит не покажет обратное.

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::num;
use super::records::{self, LIMIT_MAX};

/// Статус депозита по хешу транзакции. Ошибки: депозит не виден Bitget, не зачислен (fail), неизвестный статус.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx: &str) -> GatewayResult<Transfer> {
    let q = vec![
        ("coin", asset.ticker.clone()),
        ("limit", LIMIT_MAX.to_string()),
    ];
    let list = records::fetch(ctx, "/api/v2/spot/wallet/deposit-records", q).await?;
    let rec = list
        .iter()
        .find(|r| records::hash(r).is_some_and(|t| tx_hash::same(&t, tx)))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::DepositNotCredited,
                None,
                format!("Депозит {tx} пока не виден Bitget"),
            ))
        })?;
    let status = match num::text(rec, "status")?.as_str() {
        "success" => TransferStatus::Credited,
        "pending" => TransferStatus::Sent,
        "fail" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::DepositNotCredited,
                    None,
                    format!("Bitget: депозит {tx} не зачислен (fail)"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(super::errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    records::transfer(ctx, rec, tx, status)
}
