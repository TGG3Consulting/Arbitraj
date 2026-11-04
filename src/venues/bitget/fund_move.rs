//! Файл: venues/bitget/fund_move.rs
//! Что делает: перевод между своими счетами Bitget — спот (spot) и фонд (p2p, «P2P/funding account»): после зачисления
//! депозита — с фонда на спот (решение 102, deposit_to_trading), перед выводом — со спота на фонд. Оба — только если
//! включены настройкой: на какой счёт Bitget зачисляет депозит и с какого выводит, в документации Classic не сказано
//! (вопрос Тиграну; проверить первым депозитом и малым выводом). Деньги внутри Bitget; без ответа — «итог неизвестен»,
//! без повторов.
//! Кто вызывает: mod.rs (deposit_to_trading), transfers.rs (перед выводом).
//! От чего зависит: rest.rs, num.rs, ctx.rs, getrandom.
//! Настройки: venue.deposit_to_spot [bitget] (да — зачисленное переводить p2p → spot), venue.withdraw_from_trade
//! [bitget] (да — перед выводом переводить сумму spot → p2p).
//!
//! Официально — Transfer POST /api/v2/spot/wallet/transfer («10 times/1s (UID)»)
//! https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account#transfer — fromType*, toType* («spot:
//! Spot account», «p2p: P2P/funding account», …), amount*, coin*, clientOid («If you set duplicate clientOid, it will
//! return the result of existing clientOid transfer»); ответ data {transferId, clientOid}. Право ключа — «Transfer»
//! (код wtow в authorities, account.rs). Зачисление депозита сразу на спот можно включить и без перевода: Modify Deposit
//! Account POST /api/v2/spot/wallet/modify-deposit-account (coin, accountType spot / funding / …) — в кабинете или API.

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Счета — значения fromType / toType из документации.
pub const SPOT: &str = "spot";
pub const FUNDING: &str = "p2p";

/// Что делает: зачисленный депозит — с фонда на спот, если включено venue.deposit_to_spot.
/// Принимает: монету, сумму. Возвращает: None — выключено; Some(текст) — переведено. Ошибки: как move_funds.
pub async fn to_spot(ctx: &Ctx, asset: &Asset, amount: Decimal) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        return Ok(None);
    }
    move_funds(ctx, &asset.ticker, amount, FUNDING, SPOT)
        .await
        .map(Some)
}

/// Что делает: перед выводом — сумму со спота на фонд, если включено venue.withdraw_from_trade.
/// Принимает: тикер, сумму. Возвращает: None — выключено; Some(текст) — переведено. Ошибки: как move_funds.
pub async fn to_funding(ctx: &Ctx, ticker: &str, amount: Decimal) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.withdraw_from_trade").unwrap_or(false) {
        return Ok(None);
    }
    move_funds(ctx, ticker, amount, SPOT, FUNDING)
        .await
        .map(Some)
}

/// Что делает: один перевод между счетами.
/// Принимает: тикер, сумму, счёт-источник, счёт-получатель. Возвращает: текст для журнала (с transferId Bitget).
/// Ошибки: сумма не больше нуля, ключи, коды Bitget, OutcomeUnknown (без ответа — не повторяется).
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
    let body = json!({
        "fromType": from,
        "toType": to,
        "amount": amount.normalize().to_string(),
        "coin": ticker,
        "clientOid": client_oid()?,
    });
    let path = "/api/v2/spot/wallet/transfer";
    let data = rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Move).await?;
    let id = num::text_opt(&data, "transferId").unwrap_or_else(|| "—".into());
    Ok(format!(
        "Bitget: {} {ticker} переведено {from} → {to} (transferId {id})",
        amount.normalize()
    ))
}

/// clientOid перевода — 128 случайных бит в hex (32 знака: длина своего ID ордера по Best Practices — до 32; у перевода
/// предел не указан). Ошибка Internal — нет системного источника случайных чисел.
fn client_oid() -> GatewayResult<String> {
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
