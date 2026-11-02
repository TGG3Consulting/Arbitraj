//! Файл: venues/mexc/transfers.rs
//! Что делает: вывод с MEXC (новый эндпоинт) и его статус, статус депозита по хешу транзакции.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, core/types.
//! Настройки: нет своих.
//!
//! Официальная документация:
//! - POST /api/v3/capital/withdraw (SPOT_WITHDRAW_WRITE, вес 1) https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/withdrawnew
//!   — coin*, withdrawOrderId, netWork («Can get netWork via … config/getall … networkList»), address*, amount*;
//!   ответ {"id"}. Старый withdraw/apply — «offline soon», не используется.
//! - GET /api/v3/capital/withdraw/history (вес 1) …/withdraw-history-supporting-network — status: 1 APPLY,
//!   2 AUDITING, 3 WAIT, 4 PROCESSING, 5 WAIT_PACKAGING, 6 WAIT_CONFIRM, 7 SUCCESS, 8 FAILED, 9 CANCEL, 10 MANUAL;
//!   txId и transHash (в примере txId: null, transHash заполнен — берём любой непустой), transactionFee.
//! - GET /api/v3/capital/deposit/hisrec (вес 10) …/deposit-historysupporting-network — status: 1 SMALL, 2 TIME_DELAY,
//!   3 LARGE_DELAY, 4 PENDING, 5 SUCCESS, 6 AUDITING, 7 REJECTED, 8 REFUND, 9 PRE_SUCCESS, 10 INVALID, 11 RESTRICTED,
//!   12 COMPLETED; txId, confirmTimes, unlockConfirm.
//!   Решение 107: «зачислено» = 5 SUCCESS или 12 COMPLETED; 9 PRE_SUCCESS — ещё нет (документация не говорит, когда
//!   монеты доступны для продажи; проверить на первом депозите). 1, 2, 3, 6, 11 — ждём; 7, 8, 10 — депозит не зачислен.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Длина withdrawOrderId — документация предела не задаёт; берём как у BingX (буквы и цифры, до 32).
const MAX_WITHDRAW_ORDER_ID: usize = 32;

/// Вывод (новый эндпоинт). Принимает: запрос ядра (сеть — код netWork). Возвращает: перевод Pending с ID MEXC.
/// Ошибки: ключи, отказ MEXC (WithdrawRejected с текстом, например 10212 — адреса нет в списке разрешённых),
/// «итог неизвестен» (нет ответа).
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    let order_id: String = req
        .deal_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_WITHDRAW_ORDER_ID)
        .collect();
    if order_id.is_empty() || req.amount <= rust_decimal::Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::WithdrawRejected,
            None,
            format!(
                "Вывод не отправлен: пустой ID сделки или сумма {} не больше нуля",
                req.amount
            ),
        )));
    }
    let params = vec![
        ("coin", req.asset.ticker.clone()),
        ("withdrawOrderId", order_id),
        ("netWork", req.network.clone()),
        ("address", req.address.clone()),
        ("amount", req.amount.normalize().to_string()),
    ];
    let data = rest::signed(
        ctx,
        Method::POST,
        "/api/v3/capital/withdraw",
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
        fee: None,
        updated_at: Utc::now(),
    })
}

/// Статус вывода по ID MEXC. 1–6, 10 → Pending (ждём); 7 → Sent (есть хеш); 8, 9 → отказ с причиной.
/// Ошибки: вывод не найден, неизвестный статус, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v3/capital/withdraw/history",
        vec![("limit", "1000".to_string())],
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
            format!("Вывод {id} не найден в истории MEXC"),
        ))
    })?;
    let status = match num::int(rec, "status")? {
        1..=6 | 10 => TransferStatus::Pending,
        7 => TransferStatus::Sent,
        s @ (8 | 9) => {
            let why = if s == 8 { "FAILED" } else { "CANCEL" };
            let remark = num::text_opt(rec, "remark").unwrap_or_default();
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("MEXC: вывод {id} — {why} {remark}"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(errors::bad(
                &format!("неизвестный статус вывода {other}"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, id, status)
}

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
            .find(|r| hash(r).is_some_and(|t| same_hash(&t, tx_hash)))
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

/// Хеш транзакции записи: txId или transHash (что непустое). Ошибок нет.
fn hash(rec: &Value) -> Option<String> {
    num::text_opt(rec, "txId").or_else(|| num::text_opt(rec, "transHash"))
}

/// Тот же хеш (без учёта регистра). Ошибок нет.
fn same_hash(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Перевод формата ядра из записи истории.
fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let ticker = num::text(rec, "coin")?;
    let fee = num::dec_opt(rec, "transactionFee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f, ticker.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "network").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: hash(rec),
        fee,
        updated_at: Utc::now(),
    })
}
