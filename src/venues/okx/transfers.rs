//! Файл: venues/okx/transfers.rs
//! Что делает: вывод с OKX в сеть и его статус; поиск вывода по своему clientId при неизвестном итоге. Вывод идёт только
//! с Funding — перед ним сумма переводится с Trading (fund_move.rs, venue.withdraw_from_trade). Комиссия OKX списывается
//! сверх amt («Withdrawal fees are not included in amt»), а ядро считает «дойдёт сумма минус комиссия» и «уйдёт сумма»
//! (core/credit_expect.rs) — поэтому amt = сумма ядра − fee справочника, вниз до wdTickSz знаков.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, wallet.rs (chain, fee, minWd, wdTickSz), fund_move.rs.
//! Настройки: через fund_move.rs — venue.withdraw_from_trade, venue.transfer_poll_ms, venue.transfer_wait_ms [okx].
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Funding Account → REST API»):
//! - «Withdrawal» POST /api/v5/asset/withdrawal (6 запросов/с, User ID; право Withdraw): ccy, amt, dest «4: on-chain
//!   withdrawal», toAddr («toAddr should be a trusted address/account» — адрес должен быть в списке проверенных в WEB/APP,
//!   иначе 58207), chain, clientId («up to 32 characters», буквы и цифры); rcvrInfo — «For the specific entity users …
//!   required» (58237; для сущности Тиграна — вопрос, не отправляется); ответ wdId, clientId.
//! - «Get withdrawal history» GET /api/v5/asset/withdrawal-history (wdId, clientId): state -3 «canceling», -2 «canceled»,
//!   -1 «failed», 0 «waiting withdrawal», 1, 4–10, 12, 15–17, 19 — ожидание и проверки, 2 «success»; txId, fee, amt, chain.

use chrono::Utc;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::core_network;
use super::{fund_move, num, rest, wallet};

/// Предел clientId — официально «up to 32 characters».
const MAX_CLIENT_ID: usize = 32;
/// dest вывода в сеть — «4: on-chain withdrawal».
const ON_CHAIN: &str = "4";
const HISTORY: &str = "/api/v5/asset/withdrawal-history";

/// Вывод. Принимает: запрос ядра (сеть — сеть ядра). Возвращает: перевод Pending с wdId.
/// Ошибки: сумма меньше комиссии/минимума, перевод 18 → 6 не прошёл, отказ OKX, «итог неизвестен» (и не найден).
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    let reject = |m: String| ctx.tag(GatewayError::new(ErrorCode::WithdrawRejected, None, m));
    if req.deal_id.is_empty() || req.amount <= Decimal::ZERO {
        return Err(reject(format!(
            "Вывод не отправлен: пустой ID сделки или сумма {} не больше нуля",
            req.amount
        )));
    }
    let (chain, info) = wallet::chain(ctx, &req.asset.ticker, &req.network).await?;
    let fee = info
        .withdraw_fee
        .ok_or_else(|| reject("Вывод не отправлен: комиссия вывода (fee) не пришла".into()))?;
    let mut amt = req.amount - fee;
    if let Some(dp) = info.withdraw_precision {
        amt = amt.round_dp_with_strategy(dp, RoundingStrategy::ToZero);
    }
    if amt <= Decimal::ZERO || info.withdraw_min.is_some_and(|m| amt < m) {
        return Err(reject(format!(
            "Вывод не отправлен: {} − комиссия {fee} = {amt} {} — меньше минимума minWd {:?}",
            req.amount, req.asset.ticker, info.withdraw_min
        )));
    }
    if let Some(m) = fund_move::to_funding(ctx, &req.asset.ticker, req.amount).await? {
        ctx.log(Level::Info, "venue.transfer", m).await;
    }
    // Свой ID попытки (решение 114): миллисекунды в начале (обрезка не съест), номер сделки — для поиска.
    let attempt = Utc::now().timestamp_millis().rem_euclid(1_000_000_000);
    let client_id: String = format!("w{attempt:09}{}", req.deal_id)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_CLIENT_ID)
        .collect();
    let body = json!({
        "ccy": req.asset.ticker,
        "amt": amt.normalize().to_string(),
        "dest": ON_CHAIN,
        "toAddr": req.address,
        "chain": chain,
        "clientId": client_id,
    });
    let sent = rest::post(ctx, "/api/v5/asset/withdrawal", body, Op::Withdraw).await;
    let sent = sent.and_then(|d| {
        rest::first(&d, "withdrawal")
            .ok()
            .and_then(|r| num::text_opt(r, "wdId"))
            .ok_or_else(|| {
                let msg = format!("Вывод {client_id}: ответ без wdId — итог неизвестен");
                ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                    .with_raw(d.to_string())
            })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => match find(ctx, &client_id).await {
            Some(id) => id,
            None => return Err(e),
        },
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
        fee: Some(Cost::new(
            CostKind::WithdrawFee,
            fee,
            info.asset.ticker,
            false,
        )),
        updated_at: Utc::now(),
    })
}

/// wdId по своему clientId. None — не найден или сбой.
async fn find(ctx: &Ctx, client_id: &str) -> Option<String> {
    let data = rest::get(ctx, HISTORY, vec![("clientId", client_id.to_string())])
        .await
        .ok()?;
    data.as_array()?
        .iter()
        .find(|r| num::text_opt(r, "clientId").as_deref() == Some(client_id))
        .and_then(|r| num::text_opt(r, "wdId"))
}

/// Статус вывода по wdId: 2 → Sent (хеш txId); -1 → отказ, -2 / -3 → отмена (с состоянием); прочее → Pending.
/// Ошибки: не найден, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let data = rest::get(ctx, HISTORY, vec![("wdId", id.to_string())]).await?;
    let rec = rest::first(&data, "withdrawal-history")?;
    let status = status(rec).map_err(|m| {
        ctx.tag(GatewayError::new(
            ErrorCode::WithdrawRejected,
            None,
            format!("OKX: вывод {id} — {m}"),
        ))
        .with_raw(rec.to_string())
    })?;
    transfer(ctx, rec, id, status)
}

/// Состояние записи вывода → статус ядра. Ошибка — текст (провал или отмена).
pub fn status(rec: &Value) -> Result<TransferStatus, String> {
    match num::text_opt(rec, "state").as_deref() {
        Some("2") => Ok(TransferStatus::Sent),
        Some("-1") => Err("failed (state -1)".into()),
        Some("-2") => Err("canceled (state -2)".into()),
        Some("-3") => Err("canceling (state -3)".into()),
        Some(_) => Ok(TransferStatus::Pending),
        None => Err("в записи нет state".into()),
    }
}

/// Перевод формата ядра из записи вывода. Ошибки: нет ccy или amt.
fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let ticker = num::text(rec, "ccy")?;
    let fee_amt = num::dec_opt(rec, "fee")?;
    let fee = fee_amt.map(|f| Cost::new(CostKind::WithdrawFee, f, ticker.clone(), false));
    let chain = num::text_opt(rec, "chain").unwrap_or_default();
    Ok(Transfer {
        venue: ctx.venue(),
        network: core_network(&ticker, &chain),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        // Сумма ядра — списание целиком (amt + fee: «fees are not included in amt»), как в запросе вывода.
        amount: num::dec(rec, "amt")? + fee_amt.unwrap_or(Decimal::ZERO),
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "txId"),
        fee,
        updated_at: Utc::now(),
    })
}
