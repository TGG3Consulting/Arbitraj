//! Файл: venues/phemex/fund_move.rs
//! Что делает: перевод между своими счетами Phemex — после зачисления депозита с фьючерсного счёта (CONTRACT) на спот
//! (SPOT), решение 102 (deposit_to_trading). Только если включён настройкой: на какой счёт Phemex зачисляет депозит,
//! в документации не определено (в таблице истории depositTo — SPOT или Contract, у адреса депозита параметра счёта
//! нет) — включать после первого живого депозита. Деньги внутри Phemex; без ответа — «итог неизвестен», без повторов.
//! Кто вызывает: mod.rs (deposit_to_trading).
//! От чего зависит: rest.rs, num.rs, symbols.rs (масштаб монеты), ctx.rs.
//! Настройки: venue.deposit_to_spot [phemex] (да — зачисленное переводить CONTRACT → SPOT).
//!
//! Официально — Transfer between wallets within an account POST /wallets/account/transfer
//! (https://phemex-docs.github.io/#transfer-between-wallets-within-an-account): {fromAccType, toAccType: SPOT,
//! CONTRACT, MARGIN, COPY_TRADE; amountEv; amount; currency}; «The amountEv is the scaled-up value of the original
//! amount. Both values are required and are used to verify the correctness of the amount»; ответ data {amountEv,
//! currency, status, bizCode, createTime}. Коды status в этом разделе не расшифрованы (в старом разделе переводов
//! «0 - PENDING, 10 - DONE, 11 - FAILED») — показываются в журнале как пришли.

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, rest};

/// Счета — значения fromAccType / toAccType из документации.
pub const SPOT: &str = "SPOT";
pub const CONTRACT: &str = "CONTRACT";

/// Что делает: зачисленный депозит — с CONTRACT на SPOT, если включено venue.deposit_to_spot.
/// Принимает: справочник (масштаб), монету, сумму. Возвращает: None — выключено; Some(текст) — переведено.
/// Ошибки: как move_funds.
pub async fn to_spot(
    ctx: &Ctx,
    symbols: &Symbols,
    asset: &Asset,
    amount: Decimal,
) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        return Ok(None);
    }
    move_funds(ctx, symbols, &asset.ticker, amount, CONTRACT, SPOT)
        .await
        .map(Some)
}

/// Что делает: один перевод между счетами.
/// Принимает: справочник, тикер, сумму, счёт-источник, счёт-получатель. Возвращает: текст для журнала.
/// Ошибки: сумма не больше нуля или не ложится на масштаб, ключи, коды Phemex, OutcomeUnknown (не повторяется).
pub async fn move_funds(
    ctx: &Ctx,
    symbols: &Symbols,
    ticker: &str,
    amount: Decimal,
    from: &str,
    to: &str,
) -> GatewayResult<String> {
    let bad = |m: String| ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, m));
    if amount <= Decimal::ZERO {
        return Err(bad(format!(
            "Перевод {from} → {to} не отправлен: сумма {amount} {ticker} не больше нуля"
        )));
    }
    symbols.ensure_fresh(ctx).await?;
    let scale = symbols.scale(ctx, ticker)?;
    let amount_ev = num::scale_up(amount, scale)
        .map_err(|e| bad(format!("Перевод {from} → {to} не отправлен: {e}")))?;
    let body = json!({
        "fromAccType": from,
        "toAccType": to,
        "amountEv": amount_ev,
        "amount": amount.normalize().to_string(),
        "currency": ticker,
    });
    let path = "/wallets/account/transfer";
    let data = rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Move).await?;
    let status = num::text_opt(&data, "status").unwrap_or_else(|| "—".into());
    Ok(format!(
        "Phemex: {} {ticker} переведено {from} → {to} (status {status})",
        amount.normalize()
    ))
}
