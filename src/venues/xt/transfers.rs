//! Файл: venues/xt/transfers.rs
//! Что делает: вывод с XT.com по сети и его статус; поиск вывода по своему clientOrderId при неизвестном итоге. Оба
//! запроса — не чаще одного в секунду на ключ (общая очередь адаптера).
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, order_rules.rs (число JSON), ctx.rs, errors.rs, core/types.
//! Настройки: нет своих.
//!
//! Официально:
//! - POST /v4/withdraw (https://doc.xt.com/#deposit_withdrawalwithdraw, `_deposit_withdrawal/withdraw.md`, «1/s/apikey»):
//!   тело JSON currency, chain, amount («Withdrawal amount, including handling fee» — как считает ядро: приход = сумма −
//!   комиссия), address, memo, clientOrderId «^[a-zA-Z0-9_]{4,32}$»; ответ result {id}.
//! - GET /v4/withdraw?recordId=|clientOrderId= (#deposit_withdrawalwithdrawDetail, «1/s/apikey»): id, clientOrderId,
//!   currency, chain, address, status, amount, fee, transactionId, confirmations. Статусы — publicModule.md: SUBMIT,
//!   REVIEW (ручная проверка), AUDITED, AUDITED_AGAIN, PENDING («already on-chaining»), SUCCESS, FAIL, CANCEL.
//! - Адресная книга: коды WITHDRAW_004 «The withdrawal address is not added», WITHDRAW_016 — адрес кошелька DEX, видимо,
//!   надо добавить в кабинете заранее (в документации не описано — вопрос Тиграну). Поле type ответа в документации —
//!   CHAIN_TRANSFER/INTERNAL_TRANSFER, в примере — число 0: не читается.

use std::sync::LazyLock;
use std::time::Duration;

use chrono::Utc;
use reqwest::Method;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::{Ctx, currency, ticker};
use super::errors::{self, Op};
use super::order_rules::number;
use super::{num, rest};

/// Длина clientOrderId — из документации: «^[a-zA-Z0-9_]{4,32}$».
const MAX_CLIENT_ID: usize = 32;
/// Пауза между запросами /v4/withdraw — из документации («1/s/apikey»).
const WITHDRAW_GAP: Duration = Duration::from_millis(1000);
/// Когда /v4/withdraw (вывод или статус) запрошен последний раз (один адаптер XT на процесс).
static LAST: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Ждёт очереди /v4/withdraw (1 запрос в секунду). Ошибок нет.
async fn turn() {
    let mut last = LAST.lock().await;
    if let Some(at) = *last {
        tokio::time::sleep_until(at + WITHDRAW_GAP).await;
    }
    *last = Some(Instant::now());
}

/// Вывод. Принимает: запрос ядра (сеть — chain). Возвращает: перевод Pending с id XT.
/// Ошибки: ключи, отказ XT (WithdrawRejected — например WITHDRAW_004 «адрес не добавлен»), OutcomeUnknown — ответа нет
/// и вывод не найден по clientOrderId.
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
    // Свой id — новый на каждую попытку (метка попытки в начале, решение 114).
    let attempt = Utc::now().timestamp_millis().rem_euclid(1_000_000_000);
    let client_id: String = format!("w{attempt:09}_{}", req.deal_id)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(MAX_CLIENT_ID)
        .collect();
    let body = json!({
        "currency": currency(&req.asset.ticker),
        "chain": req.network,
        "amount": number(req.amount),
        "address": req.address,
        "clientOrderId": client_id,
    });
    turn().await;
    let sent = rest::signed(
        ctx,
        Method::POST,
        "/v4/withdraw",
        vec![],
        Some(body),
        Op::Withdraw,
    )
    .await;
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "id").ok_or_else(|| {
            let msg = format!("Вывод {client_id}: ответ без id — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match detail(ctx, ("clientOrderId", client_id.clone())).await {
                Ok(r) => match num::text_opt(&r, "id") {
                    Some(id) => id,
                    None => return Err(e),
                },
                Err(_) => return Err(e),
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

/// Запись вывода по recordId или clientOrderId (в очереди 1/с). Ошибки: связь, коды XT, пусто — не найден.
async fn detail(ctx: &Ctx, by: (&'static str, String)) -> GatewayResult<Value> {
    turn().await;
    let rec = rest::signed(
        ctx,
        Method::GET,
        "/v4/withdraw",
        vec![by.clone()],
        None,
        Op::Other,
    )
    .await?;
    if rec.is_null() || num::text_opt(&rec, "id").is_none() {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Вывод {}={} не найден на XT", by.0, by.1),
        )));
    }
    Ok(rec)
}

/// Статус вывода по id XT. SUBMIT, REVIEW, AUDITED, AUDITED_AGAIN → Pending; PENDING → Sent (если есть хеш) или
/// Pending; SUCCESS → Sent; FAIL, CANCEL → отказ. Ошибки: не найден, связь, неописанный статус.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let rec = detail(ctx, ("recordId", id.to_string())).await?;
    let st = num::text(&rec, "status")?;
    let has_hash = num::text_opt(&rec, "transactionId").is_some();
    let status = match st.as_str() {
        "SUBMIT" | "REVIEW" | "AUDITED" | "AUDITED_AGAIN" => TransferStatus::Pending,
        "PENDING" if has_hash => TransferStatus::Sent,
        "PENDING" => TransferStatus::Pending,
        "SUCCESS" => TransferStatus::Sent,
        "FAIL" | "CANCEL" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("XT: вывод {id} не выполнен (status {st})"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(errors::bad(
                &format!("статус вывода {other} не описан в документации XT — проверить вручную"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, &rec, id, status)
}

/// Перевод формата ядра из записи вывода или депозита. Ошибки: нет currency/amount.
pub(super) fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let t = ticker(&num::text(rec, "currency")?);
    let fee =
        num::dec_opt(rec, "fee")?.map(|f| Cost::new(CostKind::WithdrawFee, f, t.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker: t,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "chain").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "transactionId"),
        fee,
        updated_at: Utc::now(),
    })
}
