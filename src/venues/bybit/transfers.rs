//! Файл: venues/bybit/transfers.rs
//! Что делает: вывод с Bybit по сети (on-chain) и его статус; при неизвестном итоге — поиск своего вывода в истории
//! (монета, сеть, адрес и сумма с момента отправки; найден ровно один — его id). Статус депозита — deposits.rs.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, core/types.
//! Настройки: venue.withdraw_account [bybit] — с какого счёта выводить (FUND, UTA или FUND,UTA).
//!
//! Официальная документация:
//! - «Withdraw» POST /v5/asset/withdraw/create https://bybit-exchange.github.io/docs/v5/asset/withdraw — coin, chain,
//!   address (адрес должен быть в адресной книге: «make sure you add address in the address book first»), amount,
//!   timestamp, forceChain («1: Force the withdrawal to occur on-chain»), accountType («FUND», «UTA: System transfers the
//!   funds to Funding wallet to withdraw», «FUND,UTA,EARN» — сначала FUND), feeType («1: input amount is not the actual
//!   amount you received, the system will help to deduct the handling fee automatically»), requestId («globally unique,
//!   … idempotent verification», буквы и цифры, 1–32). Ответ result.id. «Request by the master UID's api key only»;
//!   «you can only withdraw once every 10 seconds per chain/coin combination».
//! - «Get Withdrawal Records» GET /v5/asset/withdraw/query-record …/withdraw-record — withdrawID, coin, startTime,
//!   limit [1, 50]; rows[]: withdrawId, txID («"" when withdrawal failed»), coin, chain, amount, withdrawFee, status,
//!   toAddress, createTime. enum withdrawStatus: SecurityCheck, Pending, success, CancelByUser, Reject, Fail,
//!   BlockchainConfirmed, MoreInformationRequired, Unknown («a rare status»).
//!
//! forceChain = 1 — всегда в сети (у ядра зачисление на получателе ждёт хеш транзакции); feeType = 1 — сумма вывода
//! включает комиссию, как считает ядро (приход = сумма − комиссия, core/credit_expect.rs). Оба — значения протокола.

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Длина requestId — из официальной документации («between 1 and 32 digits»).
const MAX_REQUEST_ID: usize = 32;
/// Записей на страницу истории выводов — наибольшее по документации («limit [1, 50]»).
const HISTORY_LIMIT: &str = "50";
const PATH_RECORD: &str = "/v5/asset/withdraw/query-record";

/// Вывод. Принимает: запрос ядра (сеть — chain). Возвращает: перевод Pending с id Bybit.
/// Ошибки: настройки, ключи, отказ Bybit (WithdrawRejected / InvalidAddress), «итог неизвестен».
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    let account = ctx.text("venue.withdraw_account")?;
    if req.deal_id.is_empty() || req.amount <= Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::WithdrawRejected,
            None,
            format!(
                "Вывод не отправлен: пустой ID сделки или сумма {} не больше нуля",
                req.amount
            ),
        )));
    }
    let sent_ms = Utc::now().timestamp_millis();
    // Свой ID у каждой попытки (как решение 114): метка попытки в начале — обрезка по длине её не съест.
    let request_id: String = format!("w{:09}{}", sent_ms.rem_euclid(1_000_000_000), req.deal_id)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_REQUEST_ID)
        .collect();
    let body = json!({
        "coin": req.asset.ticker,
        "chain": req.network,
        "address": req.address,
        "amount": req.amount.normalize().to_string(),
        "timestamp": sent_ms,
        "forceChain": 1,
        "accountType": account,
        "feeType": 1,
        "requestId": request_id,
    });
    let sent = rest::post(ctx, "/v5/asset/withdraw/create", &body, Op::Withdraw).await;
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "id").ok_or_else(|| {
            let msg = format!("Вывод {request_id}: ответ без id — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => match find(ctx, &req, sent_ms).await {
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

/// id своего вывода в истории: та же монета, сеть, адрес и сумма, создан не раньше отправки (startTime работает с
/// точностью до секунды — документация Deposit Records). Найден не ровно один — None. Ошибок нет (сбой — None).
async fn find(ctx: &Ctx, req: &WithdrawRequest, sent_ms: i64) -> Option<String> {
    let p = vec![
        ("coin", req.asset.ticker.clone()),
        (
            "startTime",
            (sent_ms - sent_ms.rem_euclid(1000)).to_string(),
        ),
        ("limit", HISTORY_LIMIT.to_string()),
    ];
    let data = rest::get(ctx, PATH_RECORD, p, Op::Other).await.ok()?;
    let rows = data.get("rows")?.as_array()?;
    let same: Vec<&Value> = rows
        .iter()
        .filter(|r| {
            num::text_opt(r, "chain").as_deref() == Some(req.network.as_str())
                && num::text_opt(r, "toAddress").as_deref() == Some(req.address.as_str())
                && num::dec_opt(r, "amount").ok().flatten() == Some(req.amount.normalize())
        })
        .collect();
    match same.as_slice() {
        [one] => num::text_opt(one, "withdrawId"),
        _ => None,
    }
}

/// Статус вывода по id Bybit. SecurityCheck, Pending, MoreInformationRequired, Unknown и прочие промежуточные →
/// Pending; success, BlockchainConfirmed → Sent (хеш txID); CancelByUser, Reject, Fail → отказ.
/// Ошибки: вывод не найден, связь, формат.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let p = vec![("withdrawID", id.to_string())];
    let data = rest::get(ctx, PATH_RECORD, p, Op::Other).await?;
    let rec = num::arr(&data, "rows")?
        .iter()
        .find(|r| num::text_opt(r, "withdrawId").as_deref() == Some(id))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Вывод {id} не найден в истории Bybit"),
            ))
        })?;
    let st = num::text(rec, "status")?;
    let status = match st.as_str() {
        "success" | "BlockchainConfirmed" => TransferStatus::Sent,
        "CancelByUser" | "Reject" | "Fail" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("Bybit: вывод {id} — {st}"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        _ => TransferStatus::Pending,
    };
    let ticker = num::text(rec, "coin")?;
    let fee = num::dec_opt(rec, "withdrawFee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f, ticker.clone(), false));
    let tx_hash = num::text_opt(rec, "txID");
    if status == TransferStatus::Sent && tx_hash.is_none() {
        return Err(errors::bad(
            &format!("вывод {id} {st} без txID"),
            &rec.to_string(),
        ));
    }
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "chain").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: id.to_string(),
        tx_hash,
        fee,
        updated_at: Utc::now(),
    })
}
