//! Файл: venues/bitget/transfers.rs
//! Что делает: вывод с Bitget по сети (on_chain) со своим clientOid (новым на каждую попытку) и его статус; при
//! неизвестном итоге — поиск вывода по clientOid. Перед выводом, если включено, сумма переводится со спота на фонд
//! (fund_move.rs).
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, records.rs, num.rs, ctx.rs, errors.rs, fund_move.rs, core/types.
//! Настройки: venue.history_window_s [bitget] (поиск и статус вывода — проверяется ДО отправки, урок 7); через
//! fund_move.rs — venue.withdraw_from_trade [bitget].
//!
//! Официально (https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account):
//! - Withdraw POST /api/v2/spot/wallet/withdrawal («5 req/sec/UID»; право Withdraw — wwow; «withdraw coins through a
//!   whitelisted IP address») — coin*, transferType* on_chain, address*, chain (обязателен для on_chain), size*,
//!   clientOid; ответ data {orderId, clientOid}. Адрес должен быть в адресной книге кабинета, иначе код 11000
//!   «Withdrawal whitelist is enabled» (урок 19). Входит ли комиссия в size или списывается сверху — не описано
//!   (вопрос Тиграну, проверить малым выводом).
//! - Get Withdrawal Records GET /api/v2/spot/wallet/withdrawal-records — orderId / clientOid («when passed, other
//!   parameters are ignored»; startTime и endTime всё равно обязательны); status «pending: Pending preliminary
//!   examination», «fail», «success»; tradeId — хеш при dest = on_chain; fee.

use chrono::Utc;
use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::{Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::{fund_move, num, order_rules, records, rest};

/// Путь истории выводов.
const RECORDS: &str = "/api/v2/spot/wallet/withdrawal-records";

/// Вывод. Принимает: запрос ядра (сеть — chain справочника). Возвращает: перевод Pending с orderId Bitget.
/// Ошибки: окно поиска не задано, перевод spot → p2p не прошёл, отказ Bitget, «итог неизвестен» (и не найден).
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
    records::window(ctx)?;
    if let Some(m) = fund_move::to_funding(ctx, &req.asset.ticker, req.amount).await? {
        ctx.log(Level::Info, "venue.transfer", m).await;
    }
    let client_oid = order_rules::client_oid('w', &req.deal_id);
    let body = json!({
        "coin": req.asset.ticker,
        "transferType": "on_chain",
        "address": req.address,
        "chain": req.network,
        "size": req.amount.normalize().to_string(),
        "clientOid": client_oid,
    });
    let path = "/api/v2/spot/wallet/withdrawal";
    let sent = rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Withdraw).await;
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "orderId").ok_or_else(|| {
            let msg = format!("Вывод {client_oid}: ответ без orderId — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            // Решение 231: «чем найти» — свой clientOid (ядро поищет ещё, outcome_resolve.rs).
            let e = e.pending(PendingRef::withdraw(&client_oid));
            match find(ctx, ("clientOid", client_oid.clone())).await {
                Some(r) => num::text_opt(&r, "orderId").ok_or(e)?,
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

/// Запись вывода по orderId или clientOid в окне. None — не найдена или сбой.
pub(super) async fn find(ctx: &Ctx, by: (&'static str, String)) -> Option<Value> {
    records::fetch(ctx, RECORDS, vec![by])
        .await
        .ok()?
        .into_iter()
        .next()
}

/// Статус вывода по orderId. pending → Pending; success → Sent (хеш tradeId); fail → отказ.
/// Ошибки: не найден, неизвестный статус, окно не задано, связь.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let list = records::fetch(ctx, RECORDS, vec![("orderId", id.to_string())]).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "orderId").as_deref() == Some(id))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Bitget: вывод {id} не найден в истории (окно venue.history_window_s)"),
            ))
        })?;
    let status = match num::text(rec, "status")?.as_str() {
        "pending" => TransferStatus::Pending,
        "success" => TransferStatus::Sent,
        "fail" => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("Bitget: вывод {id} — fail"),
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
    records::transfer(ctx, rec, id, status)
}
