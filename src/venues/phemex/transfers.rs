//! Файл: venues/phemex/transfers.rs
//! Что делает: вывод с Phemex по сети и его статус. Своего ID у вывода Phemex нет — при неизвестном итоге создания
//! вывод ищется в истории по адресу, сети и времени отправки; найден ровно один — он, иначе «итог неизвестен».
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, history.rs, num.rs, symbols.rs, ctx.rs, errors.rs, core/types.
//! Настройки: через history.rs — venue.history_rows [phemex] (поиск и статус вывода — проверяется ДО отправки, урок 7);
//! venue.recv_window_ms [phemex] (окно поиска вывода с неизвестным итогом — срок действия самого запроса).
//!
//! Официально (Deposit And Withdraw REST API, https://phemex-docs.github.io/):
//! - Endpoint Security Type: ключ «Read & Trade & Withdraw permission, and IP Address is mandatory», «Only main account
//!   can request withdrawal», «Only withdrawal address already added in the "withdrawal address list" on web would be
//!   allowed» — адреса вывода (кошелёк бота, адреса других бирж) заранее в кабинет (урок 19).
//! - Create withdraw request POST /phemex-withdraw/wallets/api/createWithdraw?currency=&address=&amount=&chainName=
//!   (#create-withdraw-request) — amount «withdraw amount» строкой; ответ data {id, amountRv «receive amount», feeRv,
//!   status}. Входит ли комиссия в amount, прямо не сказано (asset/info: amount — «withdraw amount with fee», есть
//!   receiveAmountRv) — проверить малым выводом, вопрос Тиграну.
//! - Withdraw status (#withdraw-status): Success «Done», Rejected, Security Checking, Security check failed, Pending
//!   Review, Address Risk, Expired, Cancelled, Pending Transfer. В примере истории — «Succeed» (противоречие с таблицей
//!   — принимаются оба, урок 2). Запроса одного вывода по id нет — ищется в истории (history.rs).

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{history, num, rest};

/// Путь истории выводов.
const HIST: &str = "/phemex-withdraw/wallets/api/withdrawHist";

/// Вывод. Принимает: запрос ядра (сеть — chainName справочника). Возвращает: перевод Pending с id Phemex.
/// Ошибки: число записей истории не задано, отказ Phemex, «итог неизвестен» (и не найден однозначно).
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
    // Урок 7: поиск вывода после отправки нужен истории — проверяется до денег.
    history::rows(ctx)?;
    let window_ms = ctx.int("venue.recv_window_ms")? as i64;
    let q = vec![
        ("currency", req.asset.ticker.clone()),
        ("address", req.address.clone()),
        ("amount", req.amount.normalize().to_string()),
        ("chainName", req.network.clone()),
    ];
    let t0 = Utc::now().timestamp_millis();
    let path = "/phemex-withdraw/wallets/api/createWithdraw";
    let sent = rest::signed(ctx, Method::POST, path, q, None, Op::Withdraw).await;
    let (venue_ref, fee) = match sent {
        Ok(d) => match num::text_opt(&d, "id") {
            Some(id) => (id, num::dec_opt(&d, "feeRv")?),
            None => {
                let msg = "Вывод Phemex: ответ без id — итог неизвестен".to_string();
                return Err(ctx
                    .tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                    .with_raw(d.to_string()));
            }
        },
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match find_sent(ctx, &req, t0 - window_ms).await {
                Some(id) => (id, None),
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    Ok(Transfer {
        venue: ctx.venue(),
        asset: req.asset.clone(),
        network: req.network,
        amount: req.amount,
        status: TransferStatus::Pending,
        venue_ref,
        tx_hash: None,
        fee: fee.map(|f| Cost::new(CostKind::WithdrawFee, f, req.asset.ticker, false)),
        updated_at: Utc::now(),
    })
}

/// Вывод с неизвестным итогом: запись той же монеты и сети на тот же адрес, поданная не раньше since (мс). Ровно одна —
/// её id; иначе None (ошибки запроса — тоже None: итог остаётся неизвестным). Ошибок нет.
async fn find_sent(ctx: &Ctx, req: &WithdrawRequest, since: i64) -> Option<String> {
    let q = vec![
        ("currency", req.asset.ticker.clone()),
        ("chainName", req.network.clone()),
    ];
    let list = history::fetch(ctx, HIST, q).await.ok()?;
    let found: Vec<&Value> = list
        .iter()
        .filter(|r| {
            num::text_opt(r, "address").is_some_and(|a| a.eq_ignore_ascii_case(&req.address))
        })
        .filter(|r| {
            r.get("submitedAt")
                .and_then(num::to_int)
                .is_some_and(|t| t >= i128::from(since))
        })
        .collect();
    match found.as_slice() {
        [one] => num::text_opt(one, "id"),
        _ => None,
    }
}

/// Статус вывода по id. Pending Transfer / Security Checking / Pending Review → Pending; Success (Succeed) → Sent (хеш
/// txHash); Cancelled → Cancelled; Rejected, Security check failed, Address Risk, Expired → отказ.
/// Ошибки: не найден, неизвестный статус, число записей не задано, связь.
pub async fn withdrawal_status(ctx: &Ctx, symbols: &Symbols, id: &str) -> GatewayResult<Transfer> {
    symbols.ensure_fresh(ctx).await?;
    let list = history::fetch(ctx, HIST, vec![]).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "id").as_deref() == Some(id))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Phemex: вывод {id} не найден в последних записях (venue.history_rows)"),
            ))
        })?;
    let status = match num::text(rec, "status")?.as_str() {
        "Pending Transfer" | "Security Checking" | "Pending Review" => TransferStatus::Pending,
        "Success" | "Succeed" => TransferStatus::Sent,
        "Cancelled" => TransferStatus::Cancelled,
        s @ ("Rejected" | "Security check failed" | "Address Risk" | "Expired") => {
            let msg = format!("Phemex: вывод {id} — {s}");
            return Err(ctx.tag(
                GatewayError::new(ErrorCode::WithdrawRejected, None, msg).with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(super::errors::bad(
                &format!("неизвестный статус вывода {other}"),
                &rec.to_string(),
            ));
        }
    };
    history::transfer(ctx, symbols, rec, id, status)
}
