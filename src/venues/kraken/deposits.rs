//! Файл: venues/kraken/deposits.rs
//! Что делает: статус депозита Kraken по хешу транзакции (список депозитов Funding (Beta) по монете — фильтра по хешу
//! нет) и удержания вывода (решение 150): депозиты в состоянии EarlyConfirmed старого DepositStatus — по справке Kraken
//! «Credited»: торговать можно, выводить ещё нельзя.
//! Кто вызывает: mod.rs (deposit_status, withdraw_hold).
//! От чего зависит: rest.rs, num.rs, methods.rs (CLASS), ctx.rs, gateway/tx_hash.rs, core/types.
//! Настройки: venue.funding_list_limit [kraken] — сколько последних депозитов читать.
//!
//! Официально:
//! - GET /funding/v1/deposits (OpenAPI spot-rest.yaml, listFundingDepositsV1): asset[class], asset[name], limit 1–500;
//!   deposits[] — deposit_id, method_id, network_id, status (initial, pending, settled, success, failure), amount {asset,
//!   amount}, fee, create_time, onchain_transaction. Смысл каждого статуса в документации не описан: зачислено — только
//!   success (торговать по settled не начинаем — проверить на живом депозите BSC); failure — не зачислен.
//! - POST /0/private/DepositStatus (Funding Legacy, схема deposit): status Initial, Pending, EarlyConfirmed, Settled,
//!   Success, Failure; status-prop return / onhold. Справка https://support.kraken.com/articles/360000674066:
//!   «Credited» — «enough confirmations… to be credited», торговать можно, выводить нельзя; соответствие Credited ↔
//!   EarlyConfirmed в документации не написано — проверить на живом депозите (вопрос в отчёте).

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, HoldItem, Transfer, TransferStatus, WithdrawHold};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::errors::Op;
use super::methods::CLASS;
use super::{num, rest};

/// Статус старого DepositStatus «зачислено, вывод ещё закрыт» — значение перечня схемы deposit.
pub const EARLY: &str = "EarlyConfirmed";

/// Хеш транзакции записи: onchain_transaction строкой; если это объект (вид в документации не описан) — строковое
/// поле объекта, совпадающее с искомым хешем. Ошибок нет.
pub fn tx_of(rec: &Value, want: Option<&str>) -> Option<String> {
    match rec.get("onchain_transaction")? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(m) => m
            .values()
            .filter_map(Value::as_str)
            .find(|s| want.is_some_and(|w| tx_hash::same(s, w)))
            .map(str::to_string),
        _ => None,
    }
}

/// Статус депозита по хешу. Ошибки: депозит не виден Kraken, не зачислен (failure), неизвестный статус.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx: &str) -> GatewayResult<Transfer> {
    let q = vec![
        rest::p("asset[class]", CLASS),
        rest::p("asset[name]", asset.ticker.clone()),
        rest::p("limit", ctx.int("venue.funding_list_limit")?.to_string()),
    ];
    let v = rest::funding(ctx, Method::GET, "/funding/v1/deposits", q, None, Op::Other).await?;
    let rows = v
        .get("deposits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rec = rows
        .iter()
        .find(|r| tx_of(r, Some(tx)).is_some_and(|t| tx_hash::same(&t, tx)))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::DepositNotCredited,
                None,
                format!("Депозит {tx} пока не виден Kraken"),
            ))
        })?;
    let status = state(rec).map_err(|e| ctx.tag(e))?;
    let (_, amount) = num::amount_of(rec.get("amount").unwrap_or(&Value::Null))?;
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker: asset.ticker.clone(),
            bsc_contract: None,
        },
        network: num::text_opt(rec, "network_id").unwrap_or_default(),
        amount,
        status,
        venue_ref: num::text_opt(rec, "deposit_id").unwrap_or_default(),
        tx_hash: tx_of(rec, Some(tx)),
        fee: None,
        updated_at: Utc::now(),
    })
}

/// Статус записи → статус ядра: success — зачислено; initial, pending, settled — ждём; failure — не зачислен.
/// Ошибки: DepositNotCredited (failure), BadResponse (неизвестный статус).
pub fn state(rec: &Value) -> GatewayResult<TransferStatus> {
    match num::text(rec, "status")?.as_str() {
        "success" => Ok(TransferStatus::Credited),
        "initial" | "pending" | "settled" => Ok(TransferStatus::Sent),
        "failure" => Err(GatewayError::new(
            ErrorCode::DepositNotCredited,
            None,
            "Kraken: депозит не зачислен (status failure)",
        )
        .with_raw(rec.to_string())),
        other => Err(super::errors::bad(
            &format!("неизвестный статус депозита {other}"),
            &rec.to_string(),
        )),
    }
}

/// Удержания вывода сейчас: депозиты EarlyConfirmed старого DepositStatus (последние по умолчанию Kraken).
/// Ошибки: ключ, права, связь, лимит, непонятный ответ.
pub async fn withdraw_hold(ctx: &Ctx) -> GatewayResult<WithdrawHold> {
    let res = rest::private(ctx, "DepositStatus", vec![], Op::Other).await?;
    hold(&res)
}

/// Разбор result DepositStatus (массив; с курсором — объект с deposit[]) в удержания: только status EarlyConfirmed.
/// Ошибка BadResponse — не массив или нет полей.
pub fn hold(res: &Value) -> GatewayResult<WithdrawHold> {
    let list = res
        .as_array()
        .or_else(|| res.get("deposit").and_then(Value::as_array))
        .ok_or_else(|| super::errors::bad("DepositStatus: result не массив", &res.to_string()))?;
    let mut items = vec![];
    for rec in list
        .iter()
        .filter(|r| num::text_opt(r, "status").as_deref() == Some(EARLY))
    {
        let asset = num::text(rec, "asset")?;
        let amount = num::dec(rec, "amount")?;
        let reason = format!(
            "зачислено досрочно, вывод ещё закрыт: {amount} {asset} (Kraken DepositStatus {EARLY}, txid {})",
            num::text_opt(rec, "txid").unwrap_or_else(|| "не указан".into())
        );
        items.push(HoldItem {
            asset,
            amount,
            reason,
        });
    }
    Ok(WithdrawHold {
        items,
        unknown: None,
    })
}
