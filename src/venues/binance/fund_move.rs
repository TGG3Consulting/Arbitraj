//! Файл: venues/binance/fund_move.rs
//! Что делает: зачисленный депозит — с фондового счёта (Funding) на спот (решение 102, deposit_to_trading), если
//! депозит Binance зачисляет на Funding (выбор в кабинете: «Кошелёк → депозит на Спот/Фонд»). Сначала читается
//! свободный остаток монеты на Funding: меньше суммы зачисления — значит, депозит пришёл на спот, переводить нечего.
//! Без ответа на перевод — «итог неизвестен», без повторов.
//! Кто вызывает: mod.rs (deposit_to_trading).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.deposit_to_spot [binance] (да — переводить; нет или не задано — не переводить).
//!
//! Официально (Wallet SAPI, официальный SDK binance-connector-rust src/wallet/rest_api):
//! - POST /sapi/v1/asset/get-funding-asset (вес 1) — asset; ответ — массив {asset, free, locked, freeze, withdrawing}.
//! - POST /sapi/v1/asset/transfer («User Universal Transfer», вес UID 300 — SDK user_universal_transfer по
//!   docs/notes/binance-analysis-2026-11-05.md; право ключа
//!   permitsUniversalTransfer) — type FUNDING_MAIN (Funding → Spot), asset, amount; ответ {"tranId"}.

use reqwest::Method;
use rust_decimal::Decimal;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Тип перевода Funding → Spot (перечень type User Universal Transfer).
const FUNDING_MAIN: &str = "FUNDING_MAIN";

/// Свободный остаток монеты на Funding. Ошибки: ключ, связь, формат.
pub async fn funding_free(ctx: &Ctx, ticker: &str) -> GatewayResult<Decimal> {
    let p = vec![("asset", ticker.to_string())];
    let path = "/sapi/v1/asset/get-funding-asset";
    let data = rest::signed(ctx, Method::POST, path, p, Op::Other).await?;
    let rows = data
        .as_array()
        .ok_or_else(|| super::errors::bad("get-funding-asset не массив", &data.to_string()))?;
    let mut free = Decimal::ZERO;
    for r in rows
        .iter()
        .filter(|r| num::text_opt(r, "asset").as_deref() == Some(ticker))
    {
        free += num::dec(r, "free")?;
    }
    Ok(free)
}

/// Что делает: депозит — с Funding на спот, если включено venue.deposit_to_spot и на Funding есть сумма зачисления.
/// Принимает: монету, сумму. Возвращает: None — переводить не нужно; Some(текст) — переведено.
/// Ошибки: сумма не больше нуля, ключи, коды Binance, OutcomeUnknown.
pub async fn to_spot(ctx: &Ctx, asset: &Asset, amount: Decimal) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        return Ok(None);
    }
    if amount <= Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Перевод Funding → Spot не отправлен: сумма {amount} {} не больше нуля",
                asset.ticker
            ),
        )));
    }
    if funding_free(ctx, &asset.ticker).await? < amount {
        return Ok(None);
    }
    let p = vec![
        ("type", FUNDING_MAIN.to_string()),
        ("asset", asset.ticker.clone()),
        ("amount", amount.normalize().to_string()),
    ];
    let data = rest::signed(ctx, Method::POST, "/sapi/v1/asset/transfer", p, Op::Move).await?;
    let id = num::text_opt(&data, "tranId").unwrap_or_else(|| "—".into());
    Ok(Some(format!(
        "Binance: {} {} переведено Funding → Spot (tranId {id})",
        amount.normalize(),
        asset.ticker
    )))
}
