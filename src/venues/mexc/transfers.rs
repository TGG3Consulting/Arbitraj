//! Файл: venues/mexc/transfers.rs
//! Что делает: вывод с MEXC (новый эндпоинт) и его статус; поиск вывода по своему withdrawOrderId при неизвестном
//! итоге (решение 118, А-🟠5). Статус депозита — deposits.rs.
//! Кто вызывает: mod.rs (функции контракта), deposits.rs (hash, transfer).
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
//! - Ответ withdraw/history содержит withdrawOrderId — по нему ищется вывод, если ответ на вывод не получен.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Длина withdrawOrderId — документация предела не задаёт; берём как у BingX (буквы и цифры, до 32).
const MAX_WITHDRAW_ORDER_ID: usize = 32;
/// Сколько записей истории выводов просить: limit — «default 1000, max 1000» (документация withdraw/history).
const HISTORY_LIMIT: &str = "1000";

/// Вывод (новый эндпоинт). Принимает: запрос ядра (сеть — код netWork). Возвращает: перевод Pending с ID MEXC.
/// Ошибки: ключи, отказ MEXC (WithdrawRejected с текстом, например 10212 — адреса нет в списке разрешённых),
/// «итог неизвестен» (нет ответа или ответ без id, и вывод не найден в истории по withdrawOrderId).
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    // Решение 118, А-🟠5 (как решение 114 у ордеров): метка попытки — миллисекунды (9 цифр) в начале, чтобы обрезка
    // по длине её не съела; номер сделки — для поиска. Правил уникальности withdrawOrderId в документации нет.
    let attempt = Utc::now().timestamp_millis().rem_euclid(1_000_000_000);
    let order_id: String = format!("w{attempt:09}{}", req.deal_id)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_WITHDRAW_ORDER_ID)
        .collect();
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
    let params = vec![
        ("coin", req.asset.ticker.clone()),
        ("withdrawOrderId", order_id.clone()),
        ("netWork", req.network.clone()),
        ("address", req.address.clone()),
        ("amount", req.amount.normalize().to_string()),
    ];
    let sent = rest::signed(
        ctx,
        Method::POST,
        "/api/v3/capital/withdraw",
        params,
        Op::Withdraw,
    )
    .await;
    // А-🟠3: HTTP успешен, а id нет — итог неизвестен (повтор дал бы второй вывод).
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "id").ok_or_else(|| {
            let msg = format!("Вывод {order_id}: ответ без id — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        // А-🟠5: итог неизвестен — поиск в истории по своему withdrawOrderId.
        Err(e) if e.code == ErrorCode::OutcomeUnknown => match find(ctx, &order_id).await {
            Some(id) => id,
            // Решение 231: «чем найти» — свой withdrawOrderId (ядро поищет ещё, outcome_resolve.rs).
            None => return Err(e.pending(PendingRef::withdraw(&order_id))),
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

/// История выводов (последние 7 дней по умолчанию документации). Ошибки: связь, коды MEXC, ответ не массив.
async fn history(ctx: &Ctx) -> GatewayResult<Vec<Value>> {
    let p = vec![("limit", HISTORY_LIMIT.to_string())];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v3/capital/withdraw/history",
        p,
        Op::Other,
    )
    .await?;
    data.as_array()
        .cloned()
        .ok_or_else(|| errors::bad("withdraw/history не массив", &data.to_string()))
}

/// ID вывода MEXC по своему withdrawOrderId (поле есть в ответе withdraw/history). None — не найден или сбой.
pub(super) async fn find(ctx: &Ctx, order_id: &str) -> Option<String> {
    let list = history(ctx).await.ok()?;
    list.iter()
        .find(|r| num::text_opt(r, "withdrawOrderId").as_deref() == Some(order_id))
        .and_then(|r| num::text_opt(r, "id"))
}

/// Статус вывода по ID MEXC. 1–6, 10 → Pending (ждём); 7 → Sent (есть хеш); 8, 9 → отказ с причиной.
/// Ошибки: вывод не найден, неизвестный статус, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let list = history(ctx).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "id").as_deref() == Some(id));
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

/// Хеш транзакции записи: txId или transHash (что непустое). Ошибок нет.
pub(super) fn hash(rec: &Value) -> Option<String> {
    num::text_opt(rec, "txId").or_else(|| num::text_opt(rec, "transHash"))
}

/// Перевод формата ядра из записи истории.
pub(super) fn transfer(
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
