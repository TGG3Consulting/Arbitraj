//! Файл: venues/gate/transfers.rs
//! Что делает: вывод с Gate на адрес в сети и его статус; поиск вывода по своему withdraw_order_id при неизвестном
//! итоге (как MEXC, решение 118, А-🟠5). Статус депозита — deposits.rs.
//! Кто вызывает: mod.rs (функции контракта), deposits.rs (transfer).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, withdraw_notice.rs, core/types.
//! Настройки: нет своих (сеть — из запроса ядра: venue.bsc_network [gate]).
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ (SDK https://github.com/gate/gateapi-python):
//! - POST /withdrawals («Withdraw», WithdrawalApi.md#withdraw; право «withdrawal»): тело LedgerRecord — currency*,
//!   amount*, address, chain* («Name of the chain used in withdrawals»), memo, withdraw_order_id; ответ — запись с id
//!   («Withdrawal record ID starts with 'w'») и status; «Withdrawal request accepted. Check withdrawal record status».
//! - GET /wallet/withdrawals (WalletApi.md#list_withdrawals; право «wallet»): withdraw_id («only this specific
//!   withdrawal record will be queried»), withdraw_order_id; запись WithdrawalRecord — txid, fee, status: BCODE, CANCEL,
//!   CANCELPEND, DONE, EXTPEND («Sent and Waiting for Confirmation»), FAIL («On-Chain Failure Pending Confirmation»),
//!   FVERIFY, LOCKED, MANUAL, REJECT, REQUEST, REVIEW (у LedgerRecord ещё INVALID, VERIFY, PROCES, PEND, DMOVE).
//!   withdraw_order_id — «up to 32 length and can only include 0-9, A-Z, a-z, underscore(_), hyphen(-) or dot(.)».
//!
//! Статусы → ядро: DONE — Sent (сеть подтвердила, есть txid); CANCEL, REJECT, INVALID — отказ с причиной fail_reason;
//! остальные — Pending (ждём). FAIL — Pending: «ожидает подтверждения», окончательный итог — следующий статус; сразу —
//! журнал и событие exec.withdraw_fail_pending, один раз на вывод (withdraw_notice.rs; решение 135, Тигран 04.11).

use chrono::Utc;
use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::withdraw_notice::Notices;
use super::{num, rest};

/// Длина withdraw_order_id — из официальной документации («up to 32 length»).
const MAX_WITHDRAW_ORDER_ID: usize = 32;

/// Вывод. Принимает: запрос ядра (сеть — chain Gate). Возвращает: перевод Pending с ID Gate («w…»).
/// Ошибки: ключи, отказ Gate (WithdrawRejected, InvalidAddress — ADDRESS_NOT_USED), «итог неизвестен».
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    if req.deal_id.is_empty() || req.amount <= rust_decimal::Decimal::ZERO {
        let m = format!(
            "Вывод не отправлен: пустой ID сделки или сумма {} не больше нуля",
            req.amount
        );
        return Err(ctx.tag(GatewayError::new(ErrorCode::WithdrawRejected, None, m)));
    }
    // Метка попытки — миллисекунды (9 цифр) в начале, чтобы обрезка по длине её не съела (как решение 114 у ордеров).
    let attempt = Utc::now().timestamp_millis().rem_euclid(1_000_000_000);
    let order_id: String = format!("w{attempt:09}{}", req.deal_id)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(MAX_WITHDRAW_ORDER_ID)
        .collect();
    let body = json!({
        "withdraw_order_id": order_id,
        "currency": req.asset.ticker,
        "address": req.address,
        "amount": req.amount.normalize().to_string(),
        "chain": req.network,
    });
    let sent = rest::signed(
        ctx,
        Method::POST,
        "/withdrawals",
        vec![],
        Some(body),
        Op::Withdraw,
    )
    .await;
    // HTTP успешен, а id нет — итог неизвестен (повтор дал бы второй вывод).
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "id").ok_or_else(|| {
            let m = format!("Вывод {order_id}: ответ без id — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, m))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => match find(ctx, &order_id).await {
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
        fee: None,
        updated_at: Utc::now(),
    })
}

/// Записи истории выводов по фильтру. Ошибки: связь, коды Gate, ответ не массив.
async fn history(ctx: &Ctx, filter: (&'static str, &str)) -> GatewayResult<Vec<Value>> {
    let p = vec![(filter.0, filter.1.to_string())];
    let data = rest::signed(ctx, Method::GET, "/wallet/withdrawals", p, None, Op::Other).await?;
    match data {
        Value::Array(a) => Ok(a),
        other => Err(errors::bad(
            "/wallet/withdrawals не массив",
            &other.to_string(),
        )),
    }
}

/// ID вывода Gate по своему withdraw_order_id. None — не найден или сбой.
async fn find(ctx: &Ctx, order_id: &str) -> Option<String> {
    let list = history(ctx, ("withdraw_order_id", order_id)).await.ok()?;
    list.iter()
        .find(|r| num::text_opt(r, "withdraw_order_id").as_deref() == Some(order_id))
        .and_then(|r| num::text_opt(r, "id"))
}

/// Статус вывода по ID Gate (см. шапку); FAIL — событие через notices. Ошибки: вывод не найден, отказ
/// (WithdrawRejected), неизвестный статус, связь.
pub async fn withdrawal_status(ctx: &Ctx, notices: &Notices, id: &str) -> GatewayResult<Transfer> {
    let list = history(ctx, ("withdraw_id", id)).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "id").as_deref() == Some(id))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Вывод {id} не найден в истории Gate"),
            ))
        })?;
    let st = num::text_opt(rec, "status").unwrap_or_default();
    let status = match st.as_str() {
        "DONE" => TransferStatus::Sent,
        "CANCEL" | "REJECT" | "INVALID" => {
            let why = num::text_opt(rec, "fail_reason").unwrap_or_default();
            let m = format!("Gate: вывод {id} — {st} {why}");
            return Err(ctx.tag(
                GatewayError::new(ErrorCode::WithdrawRejected, None, m).with_raw(rec.to_string()),
            ));
        }
        "FAIL" => {
            notices.fail_pending(ctx, id, rec).await;
            TransferStatus::Pending
        }
        "BCODE" | "CANCELPEND" | "EXTPEND" | "FVERIFY" | "LOCKED" | "MANUAL" | "REQUEST"
        | "REVIEW" | "VERIFY" | "PROCES" | "PEND" | "DMOVE" => TransferStatus::Pending,
        other => {
            return Err(errors::bad(
                &format!("неизвестный статус вывода {other}"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, id, status)
}

/// Перевод формата ядра из записи истории (вывода или депозита). Ошибки: нет currency или amount.
pub(super) fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
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
        network: num::text_opt(rec, "chain").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "txid"),
        fee,
        updated_at: Utc::now(),
    })
}
