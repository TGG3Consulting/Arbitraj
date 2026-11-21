//! Файл: venues/bsc_dex/exec/executor/fact.rs
//! Что делает: факт сделки через исполнитель (решение 122, часть X) — общий разбор квитанции exec/result.rs (события
//! Transfer: ушло из кошелька — вход, пришло на кошелёк — выход, газ, статус) плюс возврат: остаток токена входа, который
//! исполнитель вернул кошельку (пул взял меньше доли — например упёрся в предел цены), — не потрачен. Покупка: USDT
//! потрачено = ушло − вернулось (средняя цена пересчитывается); продажа: монет продано = ушло − вернулось.
//! Решение 272 (executeNative, монета сделки — нативный BNB): монеты — по событию исполнителя NativeSettled(nativeIn,
//! nativeOut): продажа — продано nativeIn − nativeOut (остаток вернулся нативным), покупка — куплено nativeOut. События
//! WBNB тут не годятся: шаг STABLE_U256_BNB сам делает withdraw/deposit исполнителем. Сторона USDT — Transfer, как раньше.
//! События нет в квитанции успешной транзакции — OutcomeUnknown с фактом (не тихий 0).
//! Кто вызывает: exec/executor/trade.rs.
//! От чего зависит: exec/{result, fill, txs}.rs, num.rs, core/types.
//! Настройки: нет.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::num;
use super::super::txs::Done;
use super::super::{fill, result};
use super::abi::ArbExecutor;

/// Итог сделки. Принимает: запрос, итог транзакции, (кошелёк, исполнитель), (токен входа, токен выхода), (decimals
/// монеты, USDT), вызов ли executeNative (решение 272). Ошибки: как у exec/result.rs::result; OutcomeUnknown — возврат
/// не переводится в число или нет события NativeSettled (факт приложен).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    d: Done,
    (wallet, executor): (Address, Address),
    (tin, tout): (Address, Address),
    decs: (u8, u8),
    native: bool,
) -> GatewayResult<TradeResult> {
    if native {
        return native_result(ctx, req, d, (wallet, executor), (tin, tout), decs);
    }
    let back_u = d
        .receipt
        .as_ref()
        .map(|r| fill::moved(r, tin, Some(executor), Some(wallet)))
        .unwrap_or_default();
    let mut t = result::result(ctx, req, d, wallet, (tin, tout), decs, &[])?;
    if back_u.is_zero() {
        return Ok(t);
    }
    let in_dec = if t.request.side == Side::Buy {
        decs.1
    } else {
        decs.0
    };
    let Some(back) = num::to_decimal(back_u, in_dec) else {
        let why = format!(
            "Обмен в блоке, но возврат исполнителя {back_u} (decimals {in_dec}) не переводится в число"
        );
        let e = ctx.err(ErrorCode::OutcomeUnknown, why);
        return Err(match t.chain.clone() {
            Some(c) => e.with_tx(c),
            None => e,
        });
    };
    let usdt = t.avg_price * t.filled_qty;
    match t.request.side {
        Side::Buy => {
            if !t.filled_qty.is_zero() {
                t.avg_price = (usdt - back) / t.filled_qty;
            }
        }
        Side::Sell => {
            t.filled_qty -= back;
            t.avg_price = if t.filled_qty > Decimal::ZERO {
                usdt / t.filled_qty
            } else {
                Decimal::ZERO
            };
            t.status = if t.filled_qty <= Decimal::ZERO {
                TradeStatus::NotFilled
            } else if t.filled_qty < t.request.qty {
                TradeStatus::PartiallyFilled
            } else {
                TradeStatus::Filled
            };
        }
    }
    Ok(t)
}

/// Итог executeNative (решение 272): общий разбор (газ, статус транзакции), монеты — по NativeSettled исполнителя,
/// USDT — по Transfer кошелька (покупка: ушло − вернулось от исполнителя). Ошибки: как у exec/result.rs::result;
/// OutcomeUnknown — транзакция в блоке без события или число не помещается (факт приложен).
fn native_result(
    ctx: &Ctx,
    req: TradeRequest,
    d: Done,
    (wallet, executor): (Address, Address),
    (tin, tout): (Address, Address),
    decs: (u8, u8),
) -> GatewayResult<TradeResult> {
    let receipt = d.receipt.clone();
    let mut t = result::result(ctx, req, d, wallet, (tin, tout), decs, &[])?;
    let unknown = |why: &str| {
        let e = ctx.err(
            ErrorCode::OutcomeUnknown,
            format!("Обмен в блоке, но {why}"),
        );
        match t.chain.clone() {
            Some(c) => e.with_tx(c),
            None => e,
        }
    };
    let Some(r) = receipt else {
        return Err(unknown("квитанции нет"));
    };
    let Some((n_in, n_out)) = settled(&r, executor) else {
        return Err(unknown("события NativeSettled исполнителя в квитанции нет"));
    };
    let (units, usdt_u) = match t.request.side {
        Side::Sell => (
            n_in.saturating_sub(n_out),
            fill::moved(&r, tout, None, Some(wallet)),
        ),
        Side::Buy => (
            n_out,
            fill::moved(&r, tin, Some(wallet), None).saturating_sub(fill::moved(
                &r,
                tin,
                Some(executor),
                Some(wallet),
            )),
        ),
    };
    let (Some(coins), Some(usdt)) = (
        num::to_decimal(units, decs.0),
        num::to_decimal(usdt_u, decs.1),
    ) else {
        return Err(unknown(&format!(
            "суммы не переводятся в числа: BNB {units} wei, USDT {usdt_u}"
        )));
    };
    t.filled_qty = coins;
    t.avg_price = if coins > Decimal::ZERO {
        usdt / coins
    } else {
        Decimal::ZERO
    };
    t.status = if coins.is_zero() {
        TradeStatus::NotFilled
    } else if coins < t.request.qty {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    Ok(t)
}

/// Событие NativeSettled(nativeIn, nativeOut) исполнителя в квитанции. None — нет. Ошибок нет.
pub fn settled(
    r: &super::super::super::send::track::Receipt,
    executor: Address,
) -> Option<(U256, U256)> {
    r.logs
        .iter()
        .filter(|l| l.address() == executor)
        .find_map(|l| ArbExecutor::NativeSettled::decode_log_data(&l.inner.data).ok())
        .map(|e| (e.nativeIn, e.nativeOut))
}
