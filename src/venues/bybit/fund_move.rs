//! Файл: venues/bybit/fund_move.rs
//! Что делает: перевод зачисленного депозита с фондового счёта Bybit (FUND) на единый торговый счёт (UNIFIED) —
//! решение 102 для Bybit. Депозит Bybit по умолчанию приходит на FUND, а спот торгует и считает баланс бот на UNIFIED
//! (account.rs): без перевода монету сделки не продать, а возвращённые USDT не видны для покупки.
//! Кто вызывает: mod.rs (deposit_to_trading контракта — после зачисления перевода, core/transfer_credit.rs).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, getrandom (UUID).
//! Настройки: venue.deposit_to_spot [bybit] (да — переводить; нет или не задано — не переводить). Решение Тиграна
//! (04.11): в кабинете Bybit депозит направлен на UNIFIED (Set Deposit Account), настройка не задаётся — этот перевод
//! остаётся запасным вариантом.
//!
//! Официальная документация:
//! - «Create Internal Transfer» POST /v5/asset/transfer/inter-transfer https://bybit-exchange.github.io/docs/v5/asset/transfer/create-inter-transfer
//!   — transferId («UUID. Please manually generate a UUID»), coin, amount, fromAccountType, toAccountType; ответ
//!   transferId, status («STATUS_UNKNOWN», «SUCCESS», «PENDING», «FAILED»). Право ключа — Wallet: AccountTransfer.
//! - enum accountType https://bybit-exchange.github.io/docs/v5/enum#accounttype — UNIFIED, FUND.
//! - «Set Deposit Account» …/asset/deposit/set-deposit-acct — «Your funds will be deposited into FUND wallet by default».

use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Счёт-источник и счёт-получатель — значения enum accountType из документации.
const FROM_ACCOUNT: &str = "FUND";
const TO_ACCOUNT: &str = "UNIFIED";

/// Переводит amount монеты с FUND на UNIFIED, если включено venue.deposit_to_spot.
/// Возвращает: None — перевод выключен; Some(текст) — переведено или принято в обработку (с transferId).
/// Ошибки: сумма не больше нуля, ключи, отказ Bybit; FAILED — BadResponse; STATUS_UNKNOWN — OutcomeUnknown.
pub async fn to_trading(
    ctx: &Ctx,
    asset: &Asset,
    amount: Decimal,
) -> GatewayResult<Option<String>> {
    if !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        return Ok(None);
    }
    transfer(ctx, asset, amount).await.map(Some)
}

/// Решение 174: тот же перевод без условия venue.deposit_to_spot — для явного действия человека (кнопка «Продать»
/// панели «Балансы», «Продать сейчас» по монете на фонде; контракт move_to_trading). Принимает: монету, сумму.
/// Возвращает: текст перевода. Ошибки: как у to_trading.
pub async fn transfer(ctx: &Ctx, asset: &Asset, amount: Decimal) -> GatewayResult<String> {
    if amount <= Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Перевод на UNIFIED не отправлен: сумма {amount} {} не больше нуля",
                asset.ticker
            ),
        )));
    }
    let id = uuid_v4()?;
    let body = json!({
        "transferId": id,
        "coin": asset.ticker,
        "amount": amount.normalize().to_string(),
        "fromAccountType": FROM_ACCOUNT,
        "toAccountType": TO_ACCOUNT,
    });
    // Деньги двигаются: без ответа итог неизвестен (OutcomeUnknown), на резервный адрес не повторяем.
    let data = rest::post(ctx, "/v5/asset/transfer/inter-transfer", &body, Op::Move).await?;
    let status = num::text_opt(&data, "status").unwrap_or_default();
    let what = format!(
        "{} {} с FUND на UNIFIED (transferId {id})",
        amount.normalize(),
        asset.ticker
    );
    match status.as_str() {
        "SUCCESS" => Ok(format!("Bybit: {what} переведено")),
        "PENDING" => Ok(format!("Bybit: {what} принято, статус PENDING")),
        "FAILED" => Err(ctx.tag(
            GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Bybit: {what} — FAILED"),
            )
            .with_raw(data.to_string()),
        )),
        other => Err(ctx.tag(
            GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!("Bybit: {what} — статус «{other}», итог неизвестен"),
            )
            .with_raw(data.to_string()),
        )),
    }
}

/// UUID версии 4 (RFC 9562, раздел 5.4: 122 случайных бита, версия 4, вариант 10) в виде 8-4-4-4-12.
/// Ошибка Internal — нет источника случайности.
fn uuid_v4() -> GatewayResult<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| {
        GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("нет случайности для UUID: {e}"),
        )
    })?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}
