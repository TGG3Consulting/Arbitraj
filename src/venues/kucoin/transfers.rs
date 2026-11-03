//! Файл: venues/kucoin/transfers.rs
//! Что делает: вывод с KuCoin (V3) и его статус; поиск вывода по своей метке remark при неизвестном итоге. Вывод идёт
//! только с main — перед ним сумма переводится с trade на main (fund_move.rs, venue.withdraw_from_trade).
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, fund_move.rs, core/types.
//! Настройки: через fund_move.rs — venue.withdraw_from_trade [kucoin].
//!
//! Официально:
//! - Withdraw (V3) POST /api/v3/withdrawals (право Withdrawal, Management, вес 5) https://www.kucoin.com/docs-new/api-3470146
//!   — currency*, chain (chainId: «it is recommended to specify the chain parameter»), amount*, toAddress*, withdrawType*
//!   ADDRESS, feeDeductType INTERNAL («Deduct the transaction fees from your withdrawal amount» — так же считает ядро:
//!   дойдёт сумма минус комиссия), remark; ответ data.withdrawalId. Своего ID вывода (clientOid) у вывода нет — метка
//!   попытки кладётся в remark (предел длины remark в документации не указан — берём предел remark ордера, 20 ASCII).
//! - Get Withdrawal History By ID GET /api/v1/withdrawals/{withdrawalId} (вес 20) https://www.kucoin.com/docs-new/api-3471890
//!   — status REVIEW, PROCESSING, WALLET_PROCESSING, SUCCESS, FAILURE; failureReason, failureReasonMsg, walletTxId, fee.
//! - Get Withdrawal History GET /api/v1/withdrawals?currency= (вес 20) https://www.kucoin.com/docs-new/api-3470145 —
//!   items[] с remark: по нему ищется вывод, если ответ на вывод не получен.

use chrono::Utc;
use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::{fund_move, num, rest};

/// Предел метки remark: предел remark ордера («length cannot exceed 20 characters (ASCII)») — у вывода не указан.
const MAX_REMARK: usize = 20;

/// Вывод. Принимает: запрос ядра (сеть — chainId). Возвращает: перевод Pending с withdrawalId.
/// Ошибки: перевод trade → main не прошёл, отказ KuCoin, «итог неизвестен» (и вывод не найден по remark).
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    if req.deal_id.is_empty() || req.amount <= rust_decimal::Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::WithdrawRejected,
            None,
            format!(
                "Вывод не отправлен: пустой ID сделки или сумма {} не больше нуля",
                req.amount
            ),
        )));
    }
    if let Some(m) = fund_move::to_main(ctx, &req.asset.ticker, req.amount).await? {
        ctx.log(Level::Info, "venue.transfer", m).await;
    }
    // Метка попытки (решение 114): миллисекунды в начале (обрезка не съест), номер сделки — для поиска.
    let attempt = Utc::now().timestamp_millis().rem_euclid(1_000_000_000);
    let remark: String = format!("w{attempt:09}{}", req.deal_id)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_REMARK)
        .collect();
    let body = json!({
        "currency": req.asset.ticker,
        "chain": req.network,
        "amount": req.amount.normalize().to_string(),
        "toAddress": req.address,
        "withdrawType": "ADDRESS",
        "feeDeductType": "INTERNAL",
        "remark": remark,
    });
    let path = "/api/v3/withdrawals";
    let sent = rest::signed(
        ctx,
        Method::POST,
        path,
        vec![],
        Some(body),
        Op::Withdraw,
        Pool::Management,
    )
    .await;
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "withdrawalId").ok_or_else(|| {
            let msg = format!("Вывод {remark}: ответ без withdrawalId — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match find(ctx, &req.asset.ticker, &remark).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    Ok(Transfer {
        venue: ctx.venue(),
        asset: req.asset,
        network: req.network,
        amount: req.amount,
        status: TransferStatus::Pending,
        venue_ref,
        tx_hash: None,
        fee: None,
        updated_at: Utc::now(),
    })
}

/// withdrawalId по своей метке remark в истории выводов монеты. None — не найден или сбой.
async fn find(ctx: &Ctx, ticker: &str, remark: &str) -> Option<String> {
    let q = vec![("currency", ticker.to_string())];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v1/withdrawals",
        q,
        None,
        Op::Other,
        Pool::Management,
    )
    .await
    .ok()?;
    data.get("items")?
        .as_array()?
        .iter()
        .find(|r| num::text_opt(r, "remark").as_deref() == Some(remark))
        .and_then(|r| num::text_opt(r, "id"))
}

/// Статус вывода по withdrawalId. REVIEW, PROCESSING, WALLET_PROCESSING → Pending; SUCCESS → Sent (хеш walletTxId);
/// FAILURE → отказ с причиной. Ошибки: не найден, неизвестный статус, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let path = format!("/api/v1/withdrawals/{id}");
    let rec = rest::signed(
        ctx,
        Method::GET,
        &path,
        vec![],
        None,
        Op::Other,
        Pool::Management,
    )
    .await?;
    let status = match num::text(&rec, "status")?.as_str() {
        "REVIEW" | "PROCESSING" | "WALLET_PROCESSING" => TransferStatus::Pending,
        "SUCCESS" => TransferStatus::Sent,
        "FAILURE" => {
            let why = num::text_opt(&rec, "failureReasonMsg")
                .or_else(|| num::text_opt(&rec, "failureReason"))
                .unwrap_or_default();
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("KuCoin: вывод {id} — FAILURE {why}"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(super::errors::bad(
                &format!("неизвестный статус вывода {other}"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, &rec, id, status, "chainId")
}

/// Хеш транзакции записи (walletTxId). Вид в документации не описан: часть после «@» (если есть) отбрасывается,
/// сравнение — gateway::tx_hash::same. Ошибок нет.
pub(super) fn hash(rec: &Value) -> Option<String> {
    num::text_opt(rec, "walletTxId").map(|h| h.split('@').next().unwrap_or_default().to_string())
}

/// Перевод формата ядра из записи (вывод или депозит). Принимает: поле сети записи («chainId» / «chain»).
pub(super) fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
    net_field: &str,
) -> GatewayResult<Transfer> {
    let ticker = num::text(rec, "currency")?;
    let fee = num::dec_opt(rec, "fee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f, ticker.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        network: num::text_opt(rec, net_field).unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: hash(rec),
        fee,
        updated_at: Utc::now(),
    })
}
