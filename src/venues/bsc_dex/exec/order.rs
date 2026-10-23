//! Файл: venues/bsc_dex/exec/order.rs
//! Что делает: обмен одной доли перед отправкой — худшая цена больше нуля, цена по живому состоянию не хуже
//! допустимой, токены, вход и минимум выхода. Минимум выхода = наибольшее из «по худшей цене» (TradeRequest.worst_price)
//! и «ожидаемое × (1 − exec.dex_slippage_pct)»; худшая цена ≤ 0 или минимум 0 — ошибка до отправки.
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs.
//! От чего зависит: exec/plan.rs (Order), pools/registry.rs, num.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex].
//!
//! «never send amountMin = 0» — руководство PancakeSwap для агентов (research/flow/bsc_dex.md, раздел 5).

use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::Entry;
use super::plan::Order;

/// Худшая допустимая цена больше нуля (иначе минимум выхода не считается). Ошибки: QuoteMismatch.
pub fn worst_positive(ctx: &Ctx, req: &TradeRequest) -> GatewayResult<()> {
    if req.worst_price <= Decimal::ZERO {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "Худшая допустимая цена {} ≤ 0 — минимум выхода не считается, сделки нет",
                req.worst_price
            ),
        ));
    }
    Ok(())
}

/// Цена по живому состоянию не хуже допустимой. Принимает: запрос, монет и USDT по расчёту.
/// Ошибки: NoRoute (ни одной монеты), QuoteMismatch (цена хуже TradeRequest.worst_price).
pub fn check_price(
    ctx: &Ctx,
    req: &TradeRequest,
    coins: Decimal,
    usdt: Decimal,
) -> GatewayResult<()> {
    if coins.is_zero() {
        return Err(ctx.err(ErrorCode::NoRoute, "Пул сейчас не даёт ни одной монеты"));
    }
    let price = usdt / coins;
    let worse = match req.side {
        Side::Buy => price > req.worst_price,
        Side::Sell => price < req.worst_price,
    };
    if worse {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "Цена в пуле сейчас {price} хуже допустимой {}",
                req.worst_price
            ),
        ));
    }
    Ok(())
}

/// Обмен одной доли: токены, вход и минимум выхода (см. шапку). Принимает: запрос, пул, монет доли (продажа),
/// (монет, USDT) по расчёту доли, (decimals монеты, USDT). Ошибки: NotConfigured (exec.dex_slippage_pct), BadResponse,
/// QuoteMismatch (худшая цена ≤ 0 или минимум выхода 0 — обмен без минимума не отправляется).
pub fn order(
    ctx: &Ctx,
    req: &TradeRequest,
    e: &Entry,
    qty: Decimal,
    (coins, usdt): (Decimal, Decimal),
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<Order> {
    worst_positive(ctx, req)?;
    let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    let o = match req.side {
        Side::Buy => Order {
            token_in: e.usdt,
            token_out: e.coin,
            amount_in: num::to_units(usdt, usdt_dec).ok_or_else(bad)?,
            min_out: num::to_units((usdt / req.worst_price).max(coins * slip), coin_dec)
                .ok_or_else(bad)?,
        },
        Side::Sell => Order {
            token_in: e.coin,
            token_out: e.usdt,
            amount_in: num::to_units(qty, coin_dec).ok_or_else(bad)?,
            min_out: num::to_units((qty * req.worst_price).max(usdt * slip), usdt_dec)
                .ok_or_else(bad)?,
        },
    };
    if o.min_out.is_zero() {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "{}: минимум выхода 0 — обмен без минимума не отправляется",
                e.family.title
            ),
        ));
    }
    Ok(o)
}
