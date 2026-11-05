//! Файл: venues/poloniex/deposits.rs
//! Что делает: статус депозита Poloniex по хешу транзакции: записи депозитов в окне venue.history_window_s
//! (activity.rs), сравнение txid с хешем (gateway::tx_hash::same).
//! Кто вызывает: mod.rs (deposit_status).
//! От чего зависит: activity.rs, num.rs, ctx.rs, gateway/tx_hash.rs.
//! Настройки: через activity.rs — venue.history_window_s [poloniex].
//!
//! Официально — Wallets Activity Records GET /wallets/activity?activityType=deposits
//! https://api-docs.poloniex.com/spot/api/private/wallet: поиска по хешу нет — ищется по txid в окне. status — в
//! документации «PENDING, COMPLETED», в SDK (polosdk/spot/rest/wallets.py) — «PENDING or COMPLETE»: принимаются оба
//! написания «зачислен» (урок 2, проверить на живом депозите). Статуса «предварительно зачислено» (как MEXC
//! PRE_SUCCESS, решение 150) в документации нет — удержания вывода (withdraw_hold) у Poloniex нет, пока живой депозит
//! не покажет обратное.

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::activity::{self, DEPOSITS};
use super::ctx::Ctx;
use super::num;

/// Статус депозита по хешу транзакции. Ошибки: депозит не виден Poloniex, неизвестный статус, окно не задано.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx: &str) -> GatewayResult<Transfer> {
    let list = activity::fetch(ctx, DEPOSITS).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "txid").is_some_and(|t| tx_hash::same(&t, tx)))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::DepositNotCredited,
                None,
                format!("Депозит {tx} пока не виден Poloniex"),
            ))
        })?;
    let status = match num::text(rec, "status")?.as_str() {
        "COMPLETED" | "COMPLETE" => TransferStatus::Credited,
        "PENDING" => TransferStatus::Sent,
        other => {
            return Err(super::errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    // Поля сети в записи депозита нет (есть currency — имя монеты, возможно в сети): сеть не заполняется.
    activity::transfer(ctx, rec, &asset.ticker, "", tx, status)
}
