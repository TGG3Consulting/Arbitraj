//! Файл: venues/coinex/deposits.rs
//! Что делает: статус депозита CoinEx по хешу транзакции: история депозитов монеты с фильтром tx_id, сравнение хеша
//! (gateway::tx_hash::same) и перевод записи в формат ядра.
//! Кто вызывает: mod.rs (deposit_status).
//! От чего зависит: rest.rs, num.rs, ctx.rs, gateway/tx_hash.rs.
//! Настройки: нет своих.
//!
//! Официально — Get Deposit History GET /assets/deposit-history?ccy=&tx_id= (подпись, группа «Query spot account history
//! (10r/1s)») https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-deposit-history: deposit_id, tx_id, ccy,
//! chain, deposit_method (on_chain / inter_user), amount, actual_amount «Actual amount deposited», confirmations, status.
//! deposit_status (https://docs.coinex.com/api/v2/enum): processing, confirming, cancelled, finished «deposit credited»,
//! too_small «lower than the minimum limit» («cannot be automatically credited, and a second deposit is needed»),
//! exception. Расхождение: в примере ответа «finish», в перечне «finished» — принимаются оба (урок 2). В примере запроса
//! «coin=CET», в таблице — ccy (берётся таблица). Статуса «предварительно зачислено» (как MEXC PRE_SUCCESS, решение 150)
//! в документации нет — withdraw_hold у CoinEx пуст (README.md адаптера).

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Статус депозита по хешу транзакции. Ошибки: депозит не виден CoinEx, не зачислен (cancelled / too_small /
/// exception), неизвестный статус, связь.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx: &str) -> GatewayResult<Transfer> {
    let q = vec![("ccy", asset.ticker.clone()), ("tx_id", tx.to_string())];
    let path = "/v2/assets/deposit-history";
    let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
    let empty = Vec::new();
    let list = data.as_array().unwrap_or(&empty);
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "tx_id").is_some_and(|t| tx_hash::same(&t, tx)))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::DepositNotCredited,
                None,
                format!("Депозит {tx} пока не виден CoinEx"),
            ))
        })?;
    let st = num::text(rec, "status")?;
    let status = match st.as_str() {
        "finished" | "finish" => TransferStatus::Credited,
        "processing" | "confirming" => TransferStatus::Sent,
        "cancelled" | "too_small" | "exception" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::DepositNotCredited,
                    None,
                    format!("CoinEx: депозит {tx} не зачислен ({st})"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(super::errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, tx, status)
}

/// Перевод формата ядра из записи депозита. Сумма — actual_amount (зачислено), нет — amount.
/// Ошибки: BadResponse — нет ccy или суммы.
fn transfer(ctx: &Ctx, rec: &Value, tx: &str, status: TransferStatus) -> GatewayResult<Transfer> {
    let amount = match num::dec_opt(rec, "actual_amount")? {
        Some(a) => a,
        None => num::dec(rec, "amount")?,
    };
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker: num::text(rec, "ccy")?.to_ascii_uppercase(),
            bsc_contract: None,
        },
        network: num::text_opt(rec, "chain").unwrap_or_default(),
        amount,
        status,
        venue_ref: tx.to_string(),
        tx_hash: num::text_opt(rec, "tx_id"),
        fee: None,
        updated_at: Utc::now(),
    })
}
