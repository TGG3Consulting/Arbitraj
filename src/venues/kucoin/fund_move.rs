//! Файл: venues/kucoin/fund_move.rs
//! Что делает: перевод между своими счетами KuCoin — основным (main, funding) и торговым (trade): после зачисления
//! депозита на main — на trade (решение 102, deposit_to_trading), перед выводом — с trade на main (вывод идёт только с
//! main). Деньги внутри KuCoin, без комиссии; без ответа — «итог неизвестен», без повторов.
//! Кто вызывает: mod.rs (deposit_to_trading), transfers.rs (перед выводом).
//! От чего зависит: rest.rs, num.rs, ctx.rs, deposit_addr.rs, getrandom.
//! Настройки: venue.deposit_to_spot [kucoin] (да — переводить зачисленное с main на trade), venue.withdraw_from_trade
//! [kucoin] (да — перед выводом переводить сумму с trade на main), venue.bsc_network (адрес депозита — на какой счёт).
//!
//! Официально:
//! - Flex Transfer POST /api/v3/accounts/universal-transfer (право FlexTransfers, пул Management, вес 4)
//!   https://www.kucoin.com/docs-new/api-3470147 — clientOid* («e.g. UUID, with a maximum length of 128 bits»),
//!   currency*, amount* («a positive integer multiple of the currency precision»), type* INTERNAL («Transfer within
//!   account»), fromAccountType* / toAccountType* (MAIN, TRADE, …); ответ data.orderId. Прежний Internal Transfer
//!   POST /api/v2/accounts/inner-transfer помечен в спецификации «abandon» — не используется.
//! - Вывод только с main: код 115007 «withdrawals only come from the funds account, so transfer funds there first»
//!   (https://www.kucoin.com/docs-new/error-code/spot); депозит по умолчанию — на MAIN («the default is MAIN»,
//!   Add Deposit Address V3).

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::deposit_addr::DepositAddrs;
use super::errors::Op;
use super::reply::Pool;
use super::{num, rest};

/// Счета — значения fromAccountType / toAccountType из документации.
pub const MAIN: &str = "MAIN";
pub const TRADE: &str = "TRADE";

/// Что делает: зачисленный депозит — с main на trade, если включено venue.deposit_to_spot и адрес депозита сети BSC
/// зачисляет на MAIN. Принимает: монету, сумму. Возвращает: None — переводить не нужно; Some(текст) — переведено.
/// Ошибки: адрес депозита, сумма, коды KuCoin, OutcomeUnknown.
pub async fn to_trade(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    asset: &Asset,
    amount: Decimal,
) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        return Ok(None);
    }
    let chain = ctx.text("venue.bsc_network")?;
    if addrs.get(ctx, asset, &chain).await?.to == TRADE {
        return Ok(None);
    }
    move_funds(ctx, &asset.ticker, amount, MAIN, TRADE)
        .await
        .map(Some)
}

/// Что делает: перед выводом — сумму с trade на main, если включено venue.withdraw_from_trade.
/// Принимает: тикер, сумму. Возвращает: None — выключено; Some(текст) — переведено. Ошибки: как move_funds.
pub async fn to_main(ctx: &Ctx, ticker: &str, amount: Decimal) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.withdraw_from_trade").unwrap_or(false) {
        return Ok(None);
    }
    move_funds(ctx, ticker, amount, TRADE, MAIN).await.map(Some)
}

/// Что делает: один перевод INTERNAL между счетами.
/// Принимает: тикер, сумму, счёт-источник, счёт-получатель. Возвращает: текст для журнала (с orderId KuCoin).
/// Ошибки: сумма не больше нуля, ключи, коды KuCoin, OutcomeUnknown (без ответа — не повторяется).
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
        "clientOid": client_oid()?,
        "currency": ticker,
        "amount": amount.normalize().to_string(),
        "type": "INTERNAL",
        "fromAccountType": from,
        "toAccountType": to,
    });
    let path = "/api/v3/accounts/universal-transfer";
    let data = rest::signed(
        ctx,
        Method::POST,
        path,
        vec![],
        Some(body),
        Op::Move,
        Pool::Management,
    )
    .await?;
    let id = num::text_opt(&data, "orderId").unwrap_or_else(|| "—".into());
    Ok(format!(
        "KuCoin: {} {ticker} переведено {from} → {to} (orderId {id})",
        amount.normalize()
    ))
}

/// clientOid — 128 случайных бит в hex (32 знака): «e.g. UUID, with a maximum length of 128 bits». Ошибка Internal —
/// нет системного источника случайных чисел.
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
