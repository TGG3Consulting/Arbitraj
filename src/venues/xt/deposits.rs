//! Файл: venues/xt/deposits.rs
//! Что делает: статус депозита XT.com по хешу транзакции: история депозитов монеты в сети BSC за окно
//! venue.history_window_s (фильтра по хешу у XT нет) — поиск записи с тем же transactionId (gateway::tx_hash::same).
//! Кто вызывает: mod.rs (deposit_status из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, transfers.rs (transfer), gateway/tx_hash.rs.
//! Настройки: venue.bsc_network, venue.history_window_s [xt].
//!
//! Официально: GET /v4/deposit/history (https://doc.xt.com/#deposit_withdrawalhistoryDepositGet,
//! `_deposit_withdrawal/historyDepositGet.md`) — currency и chain обязательны, status, fromId, direction, limit
//! («max 200»), startTime, endTime (мс); result {hasPrev, hasNext, items[{id, currency, chain, memo, status, amount,
//! confirmations, transactionId, address, fromAddr, createdTime}]}. Статусы — publicModule.md (SUBMIT, REVIEW, AUDITED,
//! PENDING, SUCCESS, FAIL, CANCEL). «Предварительно зачислено» (как MEXC PRE_SUCCESS) в документации нет; удержание
//! вывода T+1 есть только кодом отказа вывода WITHDRAW_009 — узнать его заранее через API нельзя: withdraw_hold
//! контракта у XT пустой (урок 22, вопрос Тиграну). Сколько подтверждений нужно для зачисления — не описано.

use reqwest::Method;

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::{Ctx, currency};
use super::errors::{self, Op};
use super::transfers::transfer;
use super::{num, rest};

/// Записей на страницу — наибольшее по документации («max 200»).
const PAGE: &str = "200";

/// Статус депозита по хешу. Ошибки: не виден (DepositNotCredited), не выполнен, неописанный статус, связь, настройки.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, hash: &str) -> GatewayResult<Transfer> {
    let chain = ctx.text("venue.bsc_network")?;
    let window = ctx.int("venue.history_window_s")?;
    let now = chrono::Utc::now().timestamp_millis();
    let from = now.saturating_sub(i64::try_from(window.saturating_mul(1000)).unwrap_or(i64::MAX));
    let p = vec![
        ("currency", currency(&asset.ticker)),
        ("chain", chain),
        ("limit", PAGE.to_string()),
        ("startTime", from.to_string()),
        ("endTime", now.to_string()),
    ];
    let data = rest::signed(ctx, Method::GET, "/v4/deposit/history", p, None, Op::Other).await?;
    let items = num::arr(&data, "items")?;
    let rec = items
        .iter()
        .find(|r| num::text_opt(r, "transactionId").is_some_and(|t| tx_hash::same(&t, hash)));
    let rec = rec.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::DepositNotCredited,
            None,
            format!(
                "Депозит {hash} пока не виден XT (окно venue.history_window_s, до {PAGE} записей)"
            ),
        ))
    })?;
    let st = num::text(rec, "status")?;
    let status = match st.as_str() {
        "SUCCESS" => TransferStatus::Credited,
        "SUBMIT" | "REVIEW" | "AUDITED" | "AUDITED_AGAIN" | "PENDING" => TransferStatus::Sent,
        "FAIL" | "CANCEL" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::DepositNotCredited,
                    None,
                    format!("XT: депозит {hash} не зачислен, status {st}"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(errors::bad(
                &format!("статус депозита {other} не описан в документации XT"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, hash, status)
}
