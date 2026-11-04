//! Файл: venues/okx/fund_move.rs
//! Что делает: перевод между своими счетами OKX — Funding (6) и Trading (18): после зачисления депозита на Funding — на
//! Trading (решение 102, deposit_to_trading), перед выводом — с Trading на Funding (вывод идёт только с Funding).
//! Ответ на перевод не значит «переведено» — итог проверяется GET /asset/transfer-state до success / failed. Без итога за
//! venue.transfer_wait_ms — «итог неизвестен», без повторов.
//! Кто вызывает: mod.rs (deposit_to_trading), transfers.rs (перед выводом).
//! От чего зависит: rest.rs, num.rs, ctx.rs, deposit_addr.rs, getrandom.
//! Настройки: venue.deposit_to_spot [okx] (да — переводить зачисленное 6 → 18, если адрес зачисляет на Funding),
//! venue.withdraw_from_trade [okx] (да — перед выводом переводить сумму 18 → 6), venue.transfer_poll_ms,
//! venue.transfer_wait_ms [okx] (опрос итога), venue.bsc_network (адрес депозита — на какой счёт).
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Funding Account → REST API»):
//! - «Funds transfer» POST /api/v5/asset/transfer (2 запроса/с, User ID + монета; «Only API keys with Transfer privilege
//!   can call this endpoint»): type «0: transfer within account», ccy, amt, from / to («6: Funding account 18: Trading
//!   account»), clientId («A combination of case-sensitive alphanumerics, all numbers, or all letters of up to 32
//!   characters»); ответ transId, clientId; «The success or failure of the request does not necessarily reflect the
//!   actual transfer result».
//! - «Get funds transfer state» GET /api/v5/asset/transfer-state (transId или clientId, type): state success, pending,
//!   failed.
//! - «Withdrawal»: «Only supported withdrawal of assets from funding account».

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::deposit_addr::{DepositAddrs, FUNDING, TRADING};
use super::errors::Op;
use super::{num, rest};

/// type перевода внутри своего счёта — «0: transfer within account».
const WITHIN_ACCOUNT: &str = "0";

/// Что делает: зачисленный депозит — с Funding на Trading, если включено venue.deposit_to_spot и адрес депозита сети BSC
/// зачисляет на Funding. Принимает: монету, сумму. Возвращает: None — переводить не нужно; Some(текст) — переведено.
/// Ошибки: адрес депозита, сумма, коды OKX, OutcomeUnknown.
pub async fn to_trade(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    asset: &Asset,
    amount: Decimal,
) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        return Ok(None);
    }
    let net = ctx.text("venue.bsc_network")?;
    if addrs.get(ctx, asset, &net).await?.to == TRADING {
        return Ok(None);
    }
    move_funds(ctx, &asset.ticker, amount, FUNDING, TRADING)
        .await
        .map(Some)
}

/// Что делает: перед выводом — сумму с Trading на Funding, если включено venue.withdraw_from_trade.
/// Принимает: тикер, сумму. Возвращает: None — выключено; Some(текст) — переведено. Ошибки: как move_funds.
pub async fn to_funding(ctx: &Ctx, ticker: &str, amount: Decimal) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.withdraw_from_trade").unwrap_or(false) {
        return Ok(None);
    }
    move_funds(ctx, ticker, amount, TRADING, FUNDING)
        .await
        .map(Some)
}

/// Что делает: один перевод внутри счёта и ожидание его итога.
/// Принимает: тикер, сумму, счёт-источник, счёт-получатель. Возвращает: текст для журнала (с transId OKX).
/// Ошибки: сумма не больше нуля, сроки не заданы, коды OKX, MoveFailed (failed), OutcomeUnknown (нет итога).
pub async fn move_funds(
    ctx: &Ctx,
    ticker: &str,
    amount: Decimal,
    from: &str,
    to: &str,
) -> GatewayResult<String> {
    if amount <= Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Перевод {from} → {to} не отправлен: сумма {amount} {ticker} не больше нуля"),
        )));
    }
    // Сроки опроса — до отправки: после неё поздно.
    let poll = Duration::from_millis(ctx.int("venue.transfer_poll_ms")?);
    let wait = Duration::from_millis(ctx.int("venue.transfer_wait_ms")?);
    let client_id = client_id()?;
    let body = json!({
        "type": WITHIN_ACCOUNT,
        "ccy": ticker,
        "amt": amount.normalize().to_string(),
        "from": from,
        "to": to,
        "clientId": client_id,
    });
    let sent = rest::post(ctx, "/api/v5/asset/transfer", body, Op::Move).await;
    let trans_id = sent.as_ref().ok().and_then(|d| {
        d.as_array()?
            .first()
            .and_then(|r| num::text_opt(r, "transId"))
    });
    if let Err(e) = &sent
        && e.code != ErrorCode::OutcomeUnknown
    {
        return Err(e.clone());
    }
    let q = match &trans_id {
        Some(t) => vec![("transId", t.clone()), ("type", WITHIN_ACCOUNT.into())],
        None => vec![
            ("clientId", client_id.clone()),
            ("type", WITHIN_ACCOUNT.into()),
        ],
    };
    let started = Instant::now();
    let what = format!("{} {ticker} {from} → {to}", amount.normalize());
    loop {
        let st = rest::get(ctx, "/api/v5/asset/transfer-state", q.clone()).await;
        let state = st.as_ref().ok().and_then(|d| {
            rest::first(d, "transfer-state")
                .ok()
                .and_then(|r| num::text_opt(r, "state"))
        });
        match state.as_deref() {
            Some("success") => {
                let id = trans_id
                    .clone()
                    .unwrap_or_else(|| format!("clientId {client_id}"));
                return Ok(format!("OKX: переведено {what} (transId {id})"));
            }
            Some("failed") => {
                return Err(ctx.tag(GatewayError::new(
                    ErrorCode::BadResponse,
                    None,
                    format!("OKX: перевод {what} не прошёл (transfer-state = failed)"),
                )));
            }
            _ => {}
        }
        if started.elapsed() >= wait {
            return Err(ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!("OKX: перевод {what} отправлен, итог за venue.transfer_wait_ms не получен ({state:?})"),
            )));
        }
        tokio::time::sleep(poll).await;
    }
}

/// clientId — 128 случайных бит в hex (32 знака, буквы и цифры). Ошибка Internal — нет источника случайных чисел.
fn client_id() -> GatewayResult<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| {
        GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("генератор случайных чисел: {e}"),
        )
    })?;
    Ok(hex::encode(b))
}
