//! Файл: venues/coinex/transfers.rs
//! Что делает: вывод с CoinEx по сети (on_chain) со своей меткой в remark (новой на каждую попытку) и его статус; при
//! неизвестном итоге — поиск вывода по remark на последней странице истории выводов монеты.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, order_rules.rs (client_id), ctx.rs, core/types.
//! Настройки: venue.history_page_limit [coinex] — сколько последних выводов монеты читать при поиске по remark
//! (проверяется ДО отправки, урок 7).
//!
//! Официально (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/withdrawal и …/list-withdrawal-history):
//! - Submit Withdrawal POST /assets/withdraw {ccy*, chain, to_address* («must be whitelisted»), withdraw_method on_chain,
//!   amount*, memo, extra, fee_ccy, remark «Withdrawal note»} → data {withdraw_id, …, status}. Адрес вывода — заранее в
//!   белый список на странице API-ключа (урок 19). «A withdrawal fee applies to each request, so keep enough balance» —
//!   входит ли комиссия в amount, не сказано (проверить малым выводом, вопрос Тиграну). Группа лимитов «Modify spot
//!   accounts (10r/1s)».
//! - Get Withdrawal History GET /assets/withdraw?ccy=&withdraw_id=&status=&page=&limit= (limit по умолчанию 10, максимум
//!   не указан): withdraw_id, ccy, chain, amount, actual_amount, tx_fee, tx_id, status, remark. Поиска по remark нет —
//!   перебор страницы. withdraw_status (enum): created «pending for Email confirmation», audit_required, audited,
//!   processing, confirming, finished, cancelled, cancellation_failed, failed. В примерах ответа — «audit», которого нет
//!   в перечне: принимается как ожидание (урок 2, проверить живым).

use chrono::Utc;
use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, order_rules, rest};

/// Путь вывода и его истории.
const PATH: &str = "/v2/assets/withdraw";

/// Вывод. Принимает: запрос ядра (сеть — chain справочника). Возвращает: перевод Pending с withdraw_id CoinEx.
/// Ошибки: venue.history_page_limit не задана, отказ CoinEx, «итог неизвестен» (и не найден).
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
    // Урок 7: поиск вывода после отправки нужен пределу страницы — проверяется до денег.
    let limit = ctx.int("venue.history_page_limit")?;
    let remark = order_rules::client_id('w', &req.deal_id);
    let body = json!({
        "ccy": req.asset.ticker,
        "chain": req.network,
        "to_address": req.address,
        "withdraw_method": "on_chain",
        "amount": req.amount.normalize().to_string(),
        "remark": remark,
    });
    let sent = rest::signed(ctx, Method::POST, PATH, vec![], Some(body), Op::Withdraw).await;
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "withdraw_id").ok_or_else(|| {
            let msg = format!("Вывод {remark}: ответ без withdraw_id — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match find(ctx, &req.asset.ticker, &remark, limit).await {
                Some(r) => num::text_opt(&r, "withdraw_id").ok_or(e)?,
                None => return Err(e),
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

/// Запись вывода монеты с этим remark среди последних limit. None — не найдена или сбой.
async fn find(ctx: &Ctx, ticker: &str, remark: &str, limit: u64) -> Option<Value> {
    let q = vec![("ccy", ticker.to_string()), ("limit", limit.to_string())];
    let data = rest::signed(ctx, Method::GET, PATH, q, None, Op::Other)
        .await
        .ok()?;
    data.as_array()?
        .iter()
        .find(|r| num::text_opt(r, "remark").as_deref() == Some(remark))
        .cloned()
}

/// Статус вывода по withdraw_id. Ожидание (created, audit_required, audited, processing, audit) → Pending;
/// confirming / finished → Sent (хеш tx_id); cancelled / failed → отказ; cancellation_failed (отмена не прошла — вывод
/// идёт дальше) → Pending. Ошибки: не найден, неизвестный статус, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let q = vec![("withdraw_id", id.to_string())];
    let data = rest::signed(ctx, Method::GET, PATH, q, None, Op::Other).await?;
    let rec = data
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|r| num::text_opt(r, "withdraw_id").as_deref() == Some(id))
        })
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("CoinEx: вывод {id} не найден в истории выводов"),
            ))
        })?;
    let st = num::text(rec, "status")?;
    let status = match st.as_str() {
        "created"
        | "audit_required"
        | "audited"
        | "processing"
        | "audit"
        | "cancellation_failed" => TransferStatus::Pending,
        "confirming" | "finished" => TransferStatus::Sent,
        "cancelled" | "failed" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("CoinEx: вывод {id} — {st}"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(super::errors::bad(
                &format!("неизвестный статус вывода {other}"),
                &rec.to_string(),
            ));
        }
    };
    let ticker = num::text(rec, "ccy")?.to_ascii_uppercase();
    let fee = num::dec_opt(rec, "tx_fee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f.abs(), ticker.clone(), false));
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
        tx_hash: num::text_opt(rec, "tx_id"),
        fee,
        updated_at: Utc::now(),
    })
}
