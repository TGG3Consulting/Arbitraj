//! Файл: venues/kraken/transfers.rs
//! Что делает: вывод с Kraken в сеть (Funding (Beta): метод монеты на сети BSC, address_id из адресной книги, комиссия
//! внутри суммы по котировке, сверка адреса expected_address) и его статус; поиск вывода при неизвестном итоге (своего
//! id у вывода Kraken нет). Ядро считает «уйдёт сумма, дойдёт сумма минус комиссия» (core/credit_expect.rs) — это
//! fee_included = true: gross = сумма ядра, net = сумма − комиссия. Комиссия котировки больше комиссии справочника
//! (её учло ядро) — вывод не отправляется.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, wallet.rs, withdraw_addr.rs, methods.rs, ctx.rs, errors.rs.
//! Настройки: venue.funding_list_limit [kraken].
//!
//! Официально (OpenAPI spot-rest.yaml, https://docs.kraken.com/api-reference/funding-beta/create-funding-withdrawal.md):
//! POST /funding/v1/withdrawals — scope {method_id}, address_id, amount {asset_amount {asset {class, name}, amount}},
//! fee {quoted_fee {token} | current_fee {max_fee}, fee_included}, expected_address («Must match the address stored
//! for address_id»); ответ withdrawal_id, net_amount, gross_amount, fee, approval_request_id («the withdrawal is parked
//! until the request is granted and cancelled if it is not» — кто одобряет, не описано: в журнал). Право «Funds
//! permissions - Withdraw». GET /funding/v1/withdrawals — фильтра по id нет: последние venue.funding_list_limit;
//! status pending, success, failed; onchain_transaction, method_id, address_id, create_time.

use chrono::{DateTime, Utc};
use reqwest::Method;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::deposits::tx_of;
use super::errors::Op;
use super::methods::{CLASS, Dir};
use super::{num, rest, wallet, withdraw_addr};

const PATH: &str = "/funding/v1/withdrawals";

/// Вывод. Принимает: запрос ядра (сеть — сеть ядра). Возвращает: перевод Pending с withdrawal_id.
/// Ошибки: адреса нет в книге, комиссия выросла, сумма меньше минимума, отказ Kraken, «итог неизвестен» (и не найден).
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    let reject = |m: String| ctx.tag(GatewayError::new(ErrorCode::WithdrawRejected, None, m));
    let (method_id, info) =
        wallet::method(ctx, &req.asset.ticker, &req.network, Dir::Withdraw).await?;
    let mut amount = req.amount;
    if let Some(dp) = info.withdraw_precision {
        amount = amount.round_dp_with_strategy(dp, RoundingStrategy::ToZero);
    }
    if amount <= Decimal::ZERO || info.withdraw_min.is_some_and(|m| amount < m) {
        return Err(reject(format!(
            "Вывод не отправлен: сумма {amount} {} меньше минимума метода {:?}",
            req.asset.ticker, info.withdraw_min
        )));
    }
    let (address_id, stored) = withdraw_addr::address_id(ctx, &method_id, &req.address).await?;
    let q = withdraw_addr::quote(ctx, &method_id, amount).await?;
    let known = info.withdraw_fee.unwrap_or(Decimal::ZERO);
    if info.withdraw_fee.is_none() || q.fee > known || q.net <= Decimal::ZERO {
        return Err(reject(format!(
            "Вывод не отправлен: комиссия Kraken на сумму {amount} — {} (справочник: {:?}), дойдёт {}",
            q.fee, info.withdraw_fee, q.net
        )));
    }
    let asset = json!({"class": CLASS, "name": req.asset.ticker});
    let fee = match &q.token {
        Some(t) => json!({"quoted_fee": {"token": t}, "fee_included": true}),
        None => {
            json!({"current_fee": {"max_fee": {"asset_amount": {"asset": asset, "amount": q.fee.normalize().to_string()}}}, "fee_included": true})
        }
    };
    let body = json!({
        "scope": {"method_id": method_id},
        "address_id": address_id,
        "amount": {"asset_amount": {"asset": asset, "amount": amount.normalize().to_string()}},
        "fee": fee,
        "expected_address": stored,
    });
    let started = Utc::now();
    let sent = rest::funding(ctx, Method::POST, PATH, vec![], Some(body), Op::Withdraw).await;
    let venue_ref = match sent {
        Ok(v) => match num::text_opt(&v, "withdrawal_id") {
            Some(id) => {
                if let Some(a) = num::text_opt(&v, "approval_request_id") {
                    let m = format!(
                        "Вывод {id} ждёт одобрения Kraken (approval_request_id {a}) — без одобрения отменится"
                    );
                    ctx.log(Level::Warn, "venue.transfer", m).await;
                }
                id
            }
            None => {
                let e = ctx.tag(GatewayError::new(
                    ErrorCode::OutcomeUnknown,
                    None,
                    "Вывод Kraken: ответ без withdrawal_id — итог неизвестен",
                ));
                find(ctx, &method_id, &address_id, started)
                    .await
                    .ok_or(e.with_raw(v.to_string()))?
            }
        },
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            find(ctx, &method_id, &address_id, started).await.ok_or(e)?
        }
        Err(e) => return Err(e),
    };
    Ok(Transfer {
        venue: ctx.venue(),
        asset: req.asset,
        network: req.network,
        amount,
        status: TransferStatus::Pending,
        venue_ref,
        tx_hash: None,
        fee: Some(Cost::new(
            CostKind::WithdrawFee,
            q.fee,
            info.asset.ticker,
            false,
        )),
        updated_at: Utc::now(),
    })
}

/// Последние выводы (venue.funding_list_limit). Ошибки: связь, Kraken.
async fn recent(ctx: &Ctx) -> GatewayResult<Vec<Value>> {
    let q = vec![rest::p(
        "limit",
        ctx.int("venue.funding_list_limit")?.to_string(),
    )];
    let v = rest::funding(ctx, Method::GET, PATH, q, None, Op::Other).await?;
    Ok(v.get("withdrawals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Вывод с неизвестным итогом: ровно один вывод того же метода и адреса, созданный не раньше started. None — нет
/// или неоднозначно (тогда «итог неизвестен» — ручная проверка). Ошибок нет.
async fn find(
    ctx: &Ctx,
    method_id: &str,
    address_id: &str,
    started: DateTime<Utc>,
) -> Option<String> {
    let rows = recent(ctx).await.ok()?;
    let hits: Vec<String> = rows
        .iter()
        .filter(|r| num::text_opt(r, "method_id").as_deref() == Some(method_id))
        .filter(|r| num::text_opt(r, "address_id").as_deref() == Some(address_id))
        .filter(|r| {
            num::text_opt(r, "create_time")
                .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
                .is_some_and(|t| t.with_timezone(&Utc) >= started - chrono::Duration::seconds(1))
        })
        .filter_map(|r| num::text_opt(r, "withdrawal_id"))
        .collect();
    (hits.len() == 1).then(|| hits[0].clone())
}

/// Статус вывода по withdrawal_id. Ошибки: не найден в последних, failed (WithdrawRejected), связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let rows = recent(ctx).await?;
    let rec = rows
        .iter()
        .find(|r| num::text_opt(r, "withdrawal_id").as_deref() == Some(id))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Kraken: вывода {id} нет среди последних (venue.funding_list_limit)"),
            ))
        })?;
    let tx = tx_of(rec, None);
    let status = match num::text(rec, "status")?.as_str() {
        "success" => TransferStatus::Sent,
        "pending" if tx.is_some() => TransferStatus::Sent,
        "pending" => TransferStatus::Pending,
        other => {
            let m = format!("Kraken: вывод {id} — статус {other}");
            return Err(ctx
                .tag(GatewayError::new(ErrorCode::WithdrawRejected, None, m))
                .with_raw(rec.to_string()));
        }
    };
    let (ticker, amount) = num::amount_of(rec.get("amount").unwrap_or(&Value::Null))?;
    let fee = rec
        .get("fee")
        .map(num::amount_of)
        .transpose()?
        .map(|(t, f)| Cost::new(CostKind::WithdrawFee, f, t, false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "method_id").unwrap_or_default(),
        amount,
        status,
        venue_ref: id.to_string(),
        tx_hash: tx,
        fee,
        updated_at: Utc::now(),
    })
}
