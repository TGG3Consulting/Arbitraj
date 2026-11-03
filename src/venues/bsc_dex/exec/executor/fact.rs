//! Файл: venues/bsc_dex/exec/executor/fact.rs
//! Что делает: факт сделки через исполнитель (решение 122, часть X) — общий разбор квитанции exec/result.rs (события
//! Transfer: ушло из кошелька — вход, пришло на кошелёк — выход, газ, статус) плюс возврат: остаток токена входа, который
//! исполнитель вернул кошельку (пул взял меньше доли — например упёрся в предел цены), — не потрачен. Покупка: USDT
//! потрачено = ушло − вернулось (средняя цена пересчитывается); продажа: монет продано = ушло − вернулось.
//! Кто вызывает: exec/executor/trade.rs.
//! От чего зависит: exec/{result, fill, txs}.rs, num.rs, core/types.
//! Настройки: нет.

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::num;
use super::super::txs::Done;
use super::super::{fill, result};

/// Итог сделки. Принимает: запрос, итог транзакции, (кошелёк, исполнитель), (токен входа, токен выхода), (decimals
/// монеты, USDT). Ошибки: как у exec/result.rs::result; OutcomeUnknown — возврат не переводится в число (факт приложен).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    d: Done,
    (wallet, executor): (Address, Address),
    (tin, tout): (Address, Address),
    decs: (u8, u8),
) -> GatewayResult<TradeResult> {
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
