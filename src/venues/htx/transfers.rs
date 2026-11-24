//! Файл: venues/htx/transfers.rs
//! Что делает: вывод с HTX по сети (chain HTX по сети ядра, комиссия из справочника) со своим client-order-id (новым на
//! каждую попытку) и его статус; до отправки — адрес должен быть в списке адресов вывода (добавляется только в
//! веб-интерфейсе HTX); при неизвестном итоге — поиск вывода по client-order-id.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, wallet.rs, records.rs, num.rs, order_rules.rs (client_oid), ctx.rs, errors.rs.
//! Настройки: через wallet.rs — venue.bsc_network, venue.contract_field.
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, раздел Wallet):
//! - Create a Withdraw Request POST /v1/dw/withdraw/api/create («20times/2s», право Withdraw): address*, currency*,
//!   amount*, fee («>=0», «check handling fees first»), chain (без него у USDT — ERC20: передаётся всегда), addr-tag,
//!   client-order-id («repeated client-order-id … the previously successful order ID is returned»); data — id вывода
//!   (целое). В POST amount — перевод, fee передаётся отдельно (CCXT PR 16140, HTX withdraw includeFee).
//! - Query Withdraw Address GET /v2/account/withdraw/address?currency=&chain=&limit= («[1-500]») — data[] {address,
//!   addressTag, chain, note}; адрес «must first be added in the Web UI».
//! - Query Withdrawal Order by Client Order ID GET /v1/query/withdraw/client-order-id?clientOrderId= («max 32 char»).
//! - Состояния вывода: verifying, failed, submitted, reexamine, canceled, pass, reject, pre-transfer, wallet-transfer,
//!   wallet-reject, confirmed «on-chain transfer completed with one confirmation», confirm-error «faied to get
//!   confirmation», repealed. confirm-error — итог неясен: ждём (Pending) с записью в журнал, вопрос Тиграну.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{CoinNetworkInfo, Transfer, TransferStatus, WithdrawRequest};
use crate::core::withdraw_amount;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::{Ctx, currency};
use super::errors::Op;
use super::{num, order_rules, records, rest, wallet, withdraw_address};

/// Предел длины client-order-id вывода — «max 32 char» (Query Withdrawal Order by Client Order ID).
pub const MAX_WITHDRAW_CID: usize = 32;
/// Разделитель id вывода HTX и своего client-order-id в venue_ref («<id>/<client-order-id>»).
const SEP: char = '/';

/// Вывод. Принимает: запрос ядра (сеть — сеть ядра). Возвращает: перевод Pending с «<id>/<client-order-id>».
/// Ошибки: адрес не в списке вывода, комиссия неизвестна, отказ HTX, «итог неизвестен» (и не найден).
pub async fn withdraw(
    ctx: &Ctx,
    req: WithdrawRequest,
    info: &CoinNetworkInfo,
) -> GatewayResult<Transfer> {
    if req.deal_id.is_empty() || req.amount <= rust_decimal::Decimal::ZERO {
        return Err(reject(
            ctx,
            format!(
                "Вывод не отправлен: пустой ID сделки или сумма {} не больше нуля",
                req.amount
            ),
        ));
    }
    let t = &req.asset.ticker;
    if info.venue != req.venue
        || info.asset.ticker != *t
        || info.network != req.network
        || !info.use_net_withdrawal_budget
        || !info.withdraw_enabled
        || matches!((req.asset.contract, info.asset.contract), (Some(a), Some(b)) if a != b)
    {
        return Err(super::errors::bad(
            "параметры вывода принадлежат другой сети или монете",
            "",
        ));
    }
    let amounts = withdraw_amount::amounts(req.amount, info).map_err(|e| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            e.to_string(),
        ))
    })?;
    let chain = wallet::transfer_chain(ctx, t, &req.network).await?;
    withdraw_address::check(ctx, t, &chain, &req.address).await?;
    let cid = order_rules::client_oid('w', &req.deal_id, MAX_WITHDRAW_CID);
    let submitted = super::withdraw_record::Submitted {
        currency: currency(t),
        chain: chain.coin,
        wire: amounts.wire,
        fee: amounts.fee,
    };
    let terms = serde_json::to_string(&submitted)
        .map_err(|e| super::errors::bad("не записаны параметры вывода HTX", &e.to_string()))?;
    let body = submitted.body(&req.address, &cid);
    let path = "/v1/dw/withdraw/api/create";
    let sent = rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Withdraw).await;
    let sent = sent.and_then(|d| {
        num::to_dec(&d).map(|n| n.to_string()).ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                "ответ HTX на вывод без id",
            ))
        })
    });
    let id = super::withdraw_record::accepted(sent, find_id(ctx, &cid))
        .await
        .map_err(|mut e| {
            // Ровно поля состоявшегося запроса. Остаток для торговли и лимит вывода HTX могут отличаться.
            e.message = format!(
                "{}; заявка HTX {cid} {} в сети {}: amount={}, fee={}",
                e.message,
                t,
                submitted.chain,
                submitted.wire.normalize(),
                submitted.fee.normalize()
            );
            e
        })?;
    let mut asset = req.asset;
    if asset.contract != info.asset.contract {
        asset.contract = None;
    }
    Ok(Transfer {
        venue: ctx.venue(),
        asset,
        network: req.network,
        amount: amounts.debit,
        status: TransferStatus::Pending,
        venue_ref: format!("{id}{SEP}{cid}{SEP}{terms}"),
        tx_hash: None,
        fee: None,
        updated_at: Utc::now(),
    })
}

/// Вывод по своему client-order-id. None — не найден или сбой.
async fn by_client(ctx: &Ctx, cid: &str) -> Option<Value> {
    let q = vec![("clientOrderId", cid.to_string())];
    let data = rest::get(ctx, "/v1/query/withdraw/client-order-id", q)
        .await
        .ok()?;
    data.is_object().then_some(data)
}

/// id вывода по client-order-id. None — не найден.
async fn find_id(ctx: &Ctx, cid: &str) -> Option<String> {
    num::text_opt(&by_client(ctx, cid).await?, "id")
}

/// Статус вывода по «<id>/<client-order-id>». Ошибки: не найден, отказ (провал), неизвестное состояние, связь.
pub async fn withdrawal_status(ctx: &Ctx, venue_ref: &str) -> GatewayResult<Transfer> {
    let mut parts = venue_ref.splitn(3, SEP);
    let id = parts.next().unwrap_or_default();
    let Some(cid) = parts.next() else {
        return Err(super::errors::bad(
            &format!("ссылка вывода {venue_ref} не вида «id/client-order-id»"),
            "",
        ));
    };
    let submitted = parts
        .next()
        .map(serde_json::from_str::<super::withdraw_record::Submitted>)
        .transpose()
        .map_err(|e| {
            super::errors::bad("нечитаемые параметры принятого вывода HTX", &e.to_string())
        })?;
    let rec = by_client(ctx, cid).await.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("HTX: вывод {venue_ref} не найден по client-order-id"),
        ))
    })?;
    if let Some(terms) = &submitted {
        terms.check_identity(&rec, id, cid)?;
    }
    let state = num::text(&rec, "state")?;
    let status = match state.as_str() {
        "verifying" | "submitted" | "reexamine" | "pass" | "pre-transfer" | "wallet-transfer" => {
            TransferStatus::Pending
        }
        "confirmed" => TransferStatus::Sent,
        "confirm-error" => {
            let m = format!(
                "HTX: вывод {venue_ref} — confirm-error (сеть не подтвердила) — ждём, проверьте вручную"
            );
            ctx.log(Level::Warn, "venue.transfer", m).await;
            TransferStatus::Pending
        }
        "failed" | "canceled" | "reject" | "wallet-reject" | "repealed" => {
            let why = num::text_opt(&rec, "error-msg").unwrap_or_default();
            return Err(
                reject(ctx, format!("HTX: вывод {venue_ref} — {state} {why}"))
                    .with_raw(rec.to_string()),
            );
        }
        other => {
            return Err(super::errors::bad(
                &format!("неизвестное состояние вывода {other}"),
                &rec.to_string(),
            ));
        }
    };
    let mut transfer = records::transfer(ctx, &rec, venue_ref, status)?;
    if let Some(terms) = submitted {
        transfer.amount = terms.debit_from_record(&rec)?;
        if let Some(actual) = &transfer.fee
            && actual.amount != terms.fee
        {
            ctx.log(
                Level::Warn,
                "venue.transfer",
                format!(
                    "Вывод HTX {id}, комиссия в заявке {}, факт {}, списано {}",
                    terms.fee, actual.amount, transfer.amount
                ),
            )
            .await;
        }
    }
    Ok(transfer)
}

/// Ошибка WithdrawRejected с площадкой HTX.
fn reject(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::WithdrawRejected, None, msg))
}
