//! Файл: venues/poloniex/transfers.rs
//! Что делает: вывод с Poloniex по сети (withdrawalType 0) со своим withdrawOrderId (новым на каждую попытку) и его
//! статус; при неизвестном итоге — поиск вывода по withdrawOrderId в истории (сверка расчётного круга — withdraw_recover.rs).
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, activity.rs, num.rs, ctx.rs, errors.rs, order_rules.rs (свой id), core/types.
//! Настройки: venue.history_window_s [poloniex] (поиск и статус вывода — проверяется ДО отправки, урок 7).
//!
//! Официально (https://api-docs.poloniex.com/spot/api/private/wallet):
//! - Withdraw Currency V2 POST /v2/wallets/withdraw («10» на VIP0) — coin*, amount*, network («Required for on-chain
//!   withdrawal»; в SDK — обязательный: шлётся всегда), address, addressTag (memo — не шлётся: сети с memo не
//!   поддерживаются), allowBorrow (false), withdrawOrderId («up to 32 chars», «Duplicate values are treated as
//!   idempotent submissions»), withdrawalType «0» — on-chain. Ответ {withdrawalRequestsId}. «Immediately places a
//!   withdrawal … with no email confirmation». Входит ли комиссия в amount запроса — не описано (в истории amount —
//!   «including the fee»; проверить малым выводом, вопрос Тиграну).
//! - Статус — только списком GET /wallets/activity?activityType=withdrawals (activity.rs): запроса одного вывода по id
//!   нет. status — в документации «PROCESSING, AWAITING APPROVAL, COMPLETED, COMPLETE ERROR», в SDK — «PENDING,
//!   AWAITING APPROVAL, COMPLETE, COMPLETE ERROR»: принимаются оба написания (урок 2). AWAITING APPROVAL — ручная
//!   проверка биржей (вывод ждёт). Неизвестное значение — ошибка с сырым ответом, а не угадывание.

use chrono::Utc;
use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::{Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::activity::{self, WITHDRAWALS};
use super::ctx::Ctx;
use super::errors::Op;
use super::{num, order_rules, rest};

/// Вывод. Принимает: запрос ядра (сеть — blockchain справочника). Возвращает: перевод Pending с withdrawalRequestsId.
/// Ошибки: окно поиска не задано, отказ Poloniex, «итог неизвестен» (и не найден).
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
    // Урок 7: поиск вывода после отправки нужен окну — проверяется до денег.
    activity::window(ctx)?;
    let own_id = order_rules::client_id();
    let amount = req.amount.normalize().to_string();
    let body = json!({
        "coin": req.asset.ticker,
        "network": req.network,
        "amount": amount,
        "address": req.address,
        "allowBorrow": false,
        "withdrawOrderId": own_id,
        "withdrawalType": "0",
    });
    if crate::gateway::settlement_intent::active() {
        crate::gateway::settlement_intent::before_send(
            &crate::gateway::pending::PendingRef::withdraw(&own_id),
        )
        .await?;
    }
    let sent = rest::signed(
        ctx,
        Method::POST,
        "/v2/wallets/withdraw",
        vec![],
        Some(body),
        Op::Withdraw,
    )
    .await;
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "withdrawalRequestsId").ok_or_else(|| {
            let msg = format!("Вывод {own_id}: ответ без withdrawalRequestsId — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match find(ctx, "withdrawOrderId", &own_id).await {
                Some(r) => num::text_opt(&r, "withdrawalRequestsId").ok_or(e)?,
                None => return Err(e),
            }
        }
        Err(mut e) => {
            if e.code == ErrorCode::WithdrawRejected {
                e.message
                    .push_str(&withdraw_fields(&req.asset.ticker, &req.network, &amount));
            }
            return Err(e);
        }
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

/// Only public request fields; exclude the account, signature, destination address and client ID.
fn withdraw_fields(coin: &str, network: &str, amount: &str) -> String {
    format!("; отправлено в /v2/wallets/withdraw: coin={coin}, network={network}, amount={amount}")
}

/// Запись вывода по полю (withdrawalRequestsId / withdrawOrderId) в окне. None — не найдена или сбой.
/// Вызывают: withdraw (здесь) и withdraw_recover.rs (сверка вывода расчётного круга).
pub(super) async fn find(ctx: &Ctx, field: &str, value: &str) -> Option<Value> {
    activity::fetch(ctx, WITHDRAWALS)
        .await
        .ok()?
        .into_iter()
        .find(|r| num::text_opt(r, field).as_deref() == Some(value))
}

/// Статус вывода по withdrawalRequestsId. В обработке / ждёт проверки → Pending; завершён → Sent (хеш txid);
/// COMPLETE ERROR → отказ. Ошибки: не найден, неизвестный статус, окно не задано, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let list = activity::fetch(ctx, WITHDRAWALS).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "withdrawalRequestsId").as_deref() == Some(id))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Poloniex: вывод {id} не найден в истории (окно venue.history_window_s)"),
            ))
        })?;
    let status = match num::text(rec, "status")?.as_str() {
        "PROCESSING" | "PENDING" | "AWAITING APPROVAL" => TransferStatus::Pending,
        "COMPLETED" | "COMPLETE" => TransferStatus::Sent,
        "COMPLETE ERROR" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("Poloniex: вывод {id} — COMPLETE ERROR"),
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
    let ticker = num::text(rec, "currency")?.to_ascii_uppercase();
    activity::transfer(ctx, rec, &ticker, "", id, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdrawal_refusal_shows_only_the_public_wire_fields() {
        assert_eq!(
            withdraw_fields("山野万里", "BSC", "2365410.86"),
            "; отправлено в /v2/wallets/withdraw: coin=山野万里, network=BSC, amount=2365410.86"
        );
    }
}
