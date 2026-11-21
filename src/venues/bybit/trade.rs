//! Файл: venues/bybit/trade.rs
//! Что делает: покупка или продажа на Bybit лимитным ордером с худшей допустимой ценой и сроком IOC/FOK (что не
//! исполнилось сразу — снимает биржа), проверка факта: исполнено cumExecQty, средняя цена cumExecValue / cumExecQty,
//! комиссия и её монета (fills.rs). Правила — order_rules::ready (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade из контракта).
//! Решение 268: тот же путь постановки и итога — у рыночной продажи (market_sell.rs передаёт готовый ордер, prepared).
//! От чего зависит: order_rules.rs, order_state.rs, fills.rs, rest.rs, num.rs, ctx.rs, symbols.rs.
//! Настройки: через order_rules.rs, order_state.rs, fills.rs.
//!
//! Официальная документация «Place Order» POST /v5/order/create https://bybit-exchange.github.io/docs/v5/order/create-order
//! — ответ result.orderId, result.orderLinkId; «This request is asynchronous so please use the websocket to confirm the
//! order status» — итог берётся из потока или запросом (order_state.rs). Поля итога — Get Open & Closed Orders:
//! cumExecQty, cumExecValue, qty, orderStatus, rejectReason.

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Fee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{fills, num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Bybit, Rejected без
/// исполнения), OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
/// Решение 268: `prepared` — уже подготовленный ордер (рыночная продажа, market_sell.rs): постановка и итог — тем же
/// кодом; None — обычный лимитный ордер по правилам order_rules::ready.
pub async fn trade(
    ctx: &Ctx,
    symbols: &Symbols,
    req: TradeRequest,
    prepared: Option<order_rules::Prepared>,
) -> GatewayResult<TradeResult> {
    let p = match prepared {
        Some(p) => p,
        None => order_rules::ready(ctx, symbols, &req).await?,
    };
    let placed = rest::post(ctx, "/v5/order/create", &p.body, Op::Trade).await;
    // HTTP успешен, а orderId нет — итог неизвестен (повтор дал бы второй ордер).
    let placed = placed.and_then(|r| {
        num::text_opt(&r, "orderId").ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без orderId в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())
        })
    });
    // Решение 231: «чем найти» — к любому «итог неизвестен» ниже (ядро выяснит исход само, outcome_resolve.rs).
    let pending = PendingRef::order(&p.symbol, &p.link_id, req.qty);
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.link_id).await {
                Some(id) => id,
                None => return Err(e.pending(pending)),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &p.symbol, &order_id).await;
    let out = match state {
        Ok(s) => result(ctx, req, &s, order_id.clone()).await,
        Err(e) => Err(e),
    };
    ctx.private.forget(&order_id);
    out.map_err(|e| e.pending(pending.with_ref(&order_id)))
}

/// Результат ядра по окончательной записи ордера. Ошибки: OrderRejected (Rejected без исполнения), BadResponse,
/// OutcomeUnknown (комиссия не подтверждена).
pub async fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
) -> GatewayResult<TradeResult> {
    let filled = num::dec(s, "cumExecQty")?;
    let status_text = num::text_opt(s, "orderStatus").unwrap_or_default();
    if status_text == "Rejected" && filled.is_zero() {
        let why = num::text_opt(s, "rejectReason").unwrap_or_default();
        return Err(ctx.tag(
            GatewayError::new(
                ErrorCode::OrderRejected,
                None,
                format!("Bybit: ордер {order_id} отклонён ({why})"),
            )
            .with_raw(s.to_string()),
        ));
    }
    let orig = num::dec(s, "qty")?;
    let value = num::dec_opt(s, "cumExecValue")?.unwrap_or_default();
    let status = if filled.is_zero() {
        TradeStatus::NotFilled
    } else if filled < orig {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    let avg = if filled.is_zero() {
        Decimal::ZERO
    } else {
        value / filled
    };
    let (fee, fee_asset) = if filled.is_zero() {
        (Decimal::ZERO, String::new())
    } else {
        fills::commission(ctx, &order_id, filled, s).await?
    };
    Ok(TradeResult {
        request: req,
        status,
        filled_qty: filled,
        avg_price: avg,
        fees: Fee::single(fee, fee_asset),
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}
