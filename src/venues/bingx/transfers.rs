//! Файл: venues/bingx/transfers.rs
//! Что делает: вывод с BingX, статус вывода и статус депозита.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, core/types.
//! Настройки: venue.withdraw_wallet_type.
//!
//! Официальная документация (docs-v3 и api-ai-skills/spot-wallet):
//! - POST /openApi/wallets/v1/capital/withdraw/apply — coin, network, address, amount, walletType
//!   (1 фондовый, 2 стандартные фьючерсы, 3 бессрочные фьючерсы, 4 спот), withdrawOrderId; ответ data.id.
//! - GET /openApi/api/v3/capital/withdraw/history — голый массив; status 4 на проверке, 5 ошибка, 6 выполнен; info — причина.
//! - GET /openApi/api/v3/capital/deposit/hisrec — голый массив; status по docs-v3: 0 в процессе,
//!   6 есть в сети, 1 зачислен (в api-ai-skills значения иные — см. README адаптера).

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Вывод. withdrawOrderId — идентификатор сделки ядра (только буквы и цифры, по документации).
/// Возвращает: перевод в статусе Pending с ID BingX. Ошибки: настройки, ключи, отказ BingX.
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    let wallet_type = ctx.int("venue.withdraw_wallet_type")?;
    let order_id: String = req
        .deal_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let params = vec![
        ("coin", req.asset.ticker.clone()),
        ("network", req.network.clone()),
        ("address", req.address.clone()),
        ("amount", req.amount.normalize().to_string()),
        ("walletType", wallet_type.to_string()),
        ("withdrawOrderId", order_id),
    ];
    let data = rest::signed(
        ctx,
        Method::POST,
        "/openApi/wallets/v1/capital/withdraw/apply",
        params,
        Op::Withdraw,
    )
    .await?;
    Ok(Transfer {
        venue: ctx.venue(),
        asset: req.asset,
        network: req.network,
        amount: req.amount,
        status: TransferStatus::Pending,
        venue_ref: num::text(&data, "id")?,
        tx_hash: None,
        updated_at: Utc::now(),
    })
}

/// Статус вывода по ID BingX. 4 → Pending, 5 → Rejected (причина в журнале шлюза), 6 → Sent.
/// Ошибки: вывод не найден, неизвестный статус, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let data = rest::signed(
        ctx,
        Method::GET,
        "/openApi/api/v3/capital/withdraw/history",
        vec![("id", id.to_string())],
        Op::Other,
    )
    .await?;
    let rec = data.as_array().and_then(|a| {
        a.iter()
            .find(|r| num::text_opt(r, "id").as_deref() == Some(id))
    });
    let rec = rec.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Вывод {id} не найден в истории BingX"),
        ))
    })?;
    let status = match num::int(rec, "status")? {
        4 => TransferStatus::Pending,
        5 => {
            let info = num::text_opt(rec, "info").unwrap_or_default();
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("BingX отклонил вывод {id}: {info}"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        6 => TransferStatus::Sent,
        other => {
            return Err(errors::bad(
                &format!("неизвестный статус вывода {other}"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, id, status)
}

/// Статус депозита по хешу транзакции. По docs-v3: 0 → Sent, 6 → Sent (в сети), 1 → Credited.
/// Ошибки: депозит не найден (ещё не виден бирже), неизвестный статус, связь.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
    let data = rest::signed(
        ctx,
        Method::GET,
        "/openApi/api/v3/capital/deposit/hisrec",
        vec![("coin", asset.ticker.clone())],
        Op::Other,
    )
    .await?;
    let rec = data.as_array().and_then(|a| {
        a.iter()
            .find(|r| num::text_opt(r, "txId").is_some_and(|t| t.eq_ignore_ascii_case(tx_hash)))
    });
    let rec = rec.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::DepositNotCredited,
            None,
            format!("Депозит {tx_hash} пока не виден BingX"),
        ))
    })?;
    let status = match num::int(rec, "status")? {
        0 | 6 => TransferStatus::Sent,
        1 => TransferStatus::Credited,
        other => {
            return Err(errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, tx_hash, status)
}

/// Перевод формата ядра из записи истории.
fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker: num::text(rec, "coin")?,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "network").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "txId"),
        updated_at: Utc::now(),
    })
}
