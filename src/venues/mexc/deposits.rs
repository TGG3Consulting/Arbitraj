//! Файл: venues/mexc/deposits.rs
//! Что делает: статус депозита MEXC по хешу транзакции; хеши сравниваются через gateway::tx_hash::same (нижний
//! регистр, без 0x, до «:» — решение 118, А-🟠6 / Я-16).
//! Кто вызывает: mod.rs (deposit_status из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, transfers.rs (hash, transfer), gateway/tx_hash.rs.
//! Настройки: нет своих.
//!
//! Официальная документация: GET /api/v3/capital/deposit/hisrec (вес 10)
//! https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/deposit-historysupporting-network — status: 1 SMALL,
//! 2 TIME_DELAY, 3 LARGE_DELAY, 4 PENDING, 5 SUCCESS, 6 AUDITING, 7 REJECTED, 8 REFUND, 9 PRE_SUCCESS, 10 INVALID,
//! 11 RESTRICTED, 12 COMPLETED; txId (в примере — без «0x»; формат не описан), confirmTimes, unlockConfirm.
//! Решение 107: «зачислено» = 5 SUCCESS или 12 COMPLETED; 9 PRE_SUCCESS — ещё нет (документация не говорит, когда
//! монеты доступны для продажи; проверить на первом депозите). 1, 2, 3, 6, 11 — ждём; 7, 8, 10 — депозит не зачислен.

use reqwest::Method;

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::transfers::{hash, transfer};
use super::{num, rest};

/// Статус депозита по хешу транзакции. Ошибки: депозит не найден (ещё не виден MEXC), отклонён, неизвестный статус.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v3/capital/deposit/hisrec",
        vec![("coin", asset.ticker.clone())],
        Op::Other,
    )
    .await?;
    let rec = data.as_array().and_then(|a| {
        a.iter()
            .find(|r| hash(r).is_some_and(|t| tx_hash::same(&t, tx_hash)))
    });
    let rec = rec.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::DepositNotCredited,
            None,
            format!("Депозит {tx_hash} пока не виден MEXC"),
        ))
    })?;
    let status = match num::int(rec, "status")? {
        5 | 12 => TransferStatus::Credited,
        1..=4 | 6 | 9 | 11 => TransferStatus::Sent,
        s @ (7 | 8 | 10) => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::DepositNotCredited,
                    None,
                    format!("MEXC: депозит {tx_hash} не зачислен, статус {s} (7 REJECTED, 8 REFUND, 10 INVALID)"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, tx_hash, status)
}
