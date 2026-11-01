//! Файл: venues/bingx/fund_move.rs
//! Что делает: перевод зачисленного депозита с фондового счёта BingX на спот (решение 102). Депозит по сети BSC
//! BingX зачисляет на фондовый счёт (Funding Account), а торгует и считает баланс бот на споте
//! (GET /openApi/spot/v1/account/balance, account.rs) — без перевода монеты сделки DEX → BingX не продать,
//! а возвращённые USDT не видны для покупки.
//! Кто вызывает: mod.rs (deposit_to_trading контракта — после зачисления перевода, core/transfer_credit.rs).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs.
//! Настройки: venue.deposit_to_spot [bingx] (да — переводить; нет или не задано — не переводить).
//!
//! Официальная документация (BingX api-ai-skills, skills/fund-account/api-reference.md и
//! skills/spot-account/api-reference.md, «Transfer Assets Between Account Types»):
//! - POST /openApi/api/asset/v1/transfer — fromAccount (fund — Funding Account, spot — Spot Account, …),
//!   toAccount (то же), asset (например USDT), amount; ответ data.tranId. Лимит 2 запроса/с на UID.
//! - Право ключа — «универсальный перевод» (код 4 в GET /openApi/v1/account/apiPermissions, account.rs).
//!   https://github.com/BingX-API/api-ai-skills/blob/main/skills/fund-account/api-reference.md

use reqwest::Method;
use rust_decimal::Decimal;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Счёт-источник и счёт-получатель — значения fromAccount / toAccount из документации.
const FROM_ACCOUNT: &str = "fund";
const TO_ACCOUNT: &str = "spot";

/// Переводит amount монеты с фондового счёта на спот, если включено venue.deposit_to_spot.
/// Принимает: монету (тикер BingX), сумму зачисления. Возвращает: None — перевод выключен; Some(текст) — переведено
/// (с tranId BingX). Ошибки: сумма не больше нуля, ключи, связь, отказ BingX (код и текст BingX).
pub async fn to_spot(ctx: &Ctx, asset: &Asset, amount: Decimal) -> GatewayResult<Option<String>> {
    // Не задано — функция не работает (CLAUDE.md: не задан параметр — функция не работает).
    if !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        return Ok(None);
    }
    if amount <= Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Перевод на спот не отправлен: сумма {amount} {} не больше нуля",
                asset.ticker
            ),
        )));
    }
    let params = vec![
        ("fromAccount", FROM_ACCOUNT.to_string()),
        ("toAccount", TO_ACCOUNT.to_string()),
        ("asset", asset.ticker.clone()),
        ("amount", amount.normalize().to_string()),
    ];
    let data = rest::signed(
        ctx,
        Method::POST,
        "/openApi/api/asset/v1/transfer",
        params,
        // Деньги двигаются: без ответа итог неизвестен (OutcomeUnknown), на резервный адрес не повторяем.
        Op::Move,
    )
    .await?;
    let id = num::text_opt(&data, "tranId").unwrap_or_else(|| "—".into());
    Ok(Some(format!(
        "BingX: {} {} переведено с фондового счёта на спот (tranId {id})",
        amount.normalize(),
        asset.ticker
    )))
}
