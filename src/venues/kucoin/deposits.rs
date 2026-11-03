//! Файл: venues/kucoin/deposits.rs
//! Что делает: статус депозита KuCoin по хешу транзакции (gateway::tx_hash::same; часть walletTxId после «@», если
//! есть, отбрасывается — transfers.rs::hash).
//! Кто вызывает: mod.rs (deposit_status).
//! От чего зависит: rest.rs, num.rs, ctx.rs, transfers.rs (hash, transfer), gateway/tx_hash.rs.
//! Настройки: нет своих.
//!
//! Официально — Get Deposit History GET /api/v1/deposits?currency= (Management, вес 5)
//! https://www.kucoin.com/docs-new/api-3470141: «Items are paginated and sorted to show the latest first», pageSize
//! «Minimum is 10, maximum is 500»; items[]: currency, chain, status (PROCESSING, SUCCESS, FAILURE), amount, fee,
//! walletTxId, isInner, arrears. Зачислено = SUCCESS; PROCESSING — ждём; FAILURE — не зачислен.
//! Депозит зачисляется на счёт адреса (to: MAIN по умолчанию) — перевод на trade делает deposit_to_trading.

use reqwest::Method;

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::transfers::{hash, transfer};
use super::{num, rest};

/// Наибольший pageSize истории — официально «maximum is 500».
const PAGE_SIZE: &str = "500";

/// Статус депозита по хешу транзакции. Ошибки: депозит не виден KuCoin, не зачислен (FAILURE), неизвестный статус.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx: &str) -> GatewayResult<Transfer> {
    let q = vec![
        ("currency", asset.ticker.clone()),
        ("pageSize", PAGE_SIZE.to_string()),
    ];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v1/deposits",
        q,
        None,
        Op::Other,
        Pool::Management,
    )
    .await?;
    let rec = data.get("items").and_then(|v| v.as_array()).and_then(|a| {
        a.iter()
            .find(|r| hash(r).is_some_and(|t| tx_hash::same(&t, tx)))
    });
    let rec = rec.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::DepositNotCredited,
            None,
            format!("Депозит {tx} пока не виден KuCoin"),
        ))
    })?;
    let status = match num::text(rec, "status")?.as_str() {
        "SUCCESS" => TransferStatus::Credited,
        "PROCESSING" => TransferStatus::Sent,
        "FAILURE" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::DepositNotCredited,
                    None,
                    format!("KuCoin: депозит {tx} не зачислен (FAILURE)"),
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
    transfer(ctx, rec, tx, status, "chain")
}
