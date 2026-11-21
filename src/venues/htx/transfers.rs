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
//!   (целое). «Only support the existed addresses in your withdraw address list». Входит ли fee в amount — не описано
//!   (вопрос Тиграну, проверить малым выводом).
//! - Query Withdraw Address GET /v2/account/withdraw/address?currency=&chain=&limit= («[1-500]») — data[] {address,
//!   addressTag, chain, note}; адрес «must first be added in the Web UI».
//! - Query Withdrawal Order by Client Order ID GET /v1/query/withdraw/client-order-id?clientOrderId= («max 32 char»).
//! - Состояния вывода: verifying, failed, submitted, reexamine, canceled, pass, reject, pre-transfer, wallet-transfer,
//!   wallet-reject, confirmed «on-chain transfer completed with one confirmation», confirm-error «faied to get
//!   confirmation», repealed. confirm-error — итог неясен: ждём (Pending) с записью в журнал, вопрос Тиграну.

use chrono::Utc;
use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::{Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::{Ctx, currency};
use super::errors::Op;
use super::{num, order_rules, records, rest, wallet};

/// Предел длины client-order-id вывода — «max 32 char» (Query Withdrawal Order by Client Order ID).
pub const MAX_WITHDRAW_CID: usize = 32;
/// Разделитель id вывода HTX и своего client-order-id в venue_ref («<id>/<client-order-id>»).
const SEP: char = '/';

/// Вывод. Принимает: запрос ядра (сеть — сеть ядра). Возвращает: перевод Pending с «<id>/<client-order-id>».
/// Ошибки: адрес не в списке вывода, комиссия неизвестна, отказ HTX, «итог неизвестен» (и не найден).
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
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
    let chain = wallet::chain_of(ctx, t, &req.network).await?;
    let fee = wallet::withdraw_fee(ctx, t, &req.network).await?;
    whitelisted(ctx, t, &chain, &req.address).await?;
    let cid = order_rules::client_oid('w', &req.deal_id, MAX_WITHDRAW_CID);
    let body = json!({
        "address": req.address,
        "currency": currency(t),
        "amount": req.amount.normalize().to_string(),
        "fee": fee.normalize().to_string(),
        "chain": chain,
        "client-order-id": cid,
    });
    let path = "/v1/dw/withdraw/api/create";
    let sent = rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Withdraw).await;
    let id = match sent.map(|d| num::to_dec(&d)) {
        Ok(Some(id)) => id.to_string(),
        Ok(None) => find_id(ctx, &cid).await.ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!("Вывод {cid}: ответ без id — итог неизвестен"),
            ))
        })?,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => find_id(ctx, &cid).await.ok_or(e)?,
        Err(e) => return Err(e),
    };
    Ok(Transfer {
        venue: ctx.venue(),
        asset: req.asset,
        network: req.network,
        amount: req.amount,
        status: TransferStatus::Pending,
        venue_ref: format!("{id}{SEP}{cid}"),
        tx_hash: None,
        fee: None,
        updated_at: Utc::now(),
    })
}

/// Адрес есть в списке адресов вывода монеты в сети chain. Ошибки: WithdrawRejected — нет; связь.
async fn whitelisted(ctx: &Ctx, ticker: &str, chain: &str, address: &str) -> GatewayResult<()> {
    let q = vec![
        ("currency", currency(ticker)),
        ("chain", chain.to_string()),
        ("limit", records::SIZE_MAX.to_string()),
    ];
    let data = rest::get(ctx, "/v2/account/withdraw/address", q).await?;
    let found = data.as_array().is_some_and(|a| {
        a.iter()
            .any(|r| num::text_opt(r, "address").is_some_and(|x| x.eq_ignore_ascii_case(address)))
    });
    if found {
        return Ok(());
    }
    Err(reject(
        ctx,
        format!(
            "Адреса {address} нет в списке адресов вывода HTX ({ticker}, chain {chain}) — добавьте его в веб-интерфейсе \
             HTX (через API нельзя)"
        ),
    ))
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
    let Some((_, cid)) = venue_ref.split_once(SEP) else {
        return Err(super::errors::bad(
            &format!("ссылка вывода {venue_ref} не вида «id/client-order-id»"),
            "",
        ));
    };
    let rec = by_client(ctx, cid).await.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("HTX: вывод {venue_ref} не найден по client-order-id"),
        ))
    })?;
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
    records::transfer(ctx, &rec, venue_ref, status)
}

/// Ошибка WithdrawRejected с площадкой HTX.
fn reject(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::WithdrawRejected, None, msg))
}
