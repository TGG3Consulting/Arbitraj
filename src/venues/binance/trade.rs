//! Файл: venues/binance/trade.rs
//! Что делает: покупка или продажа на Binance лимитным ордером IOC (или FOK) с худшей допустимой ценой — что не
//! исполнилось сразу, снимает биржа (статус EXPIRED). Итог берётся из ответа FULL (status, executedQty,
//! cummulativeQuoteQty, fills с комиссией); если ответ не окончательный — опрос и отмена (order_state.rs).
//! Правила пары и пробный ордер — order_rules.rs: та же функция у проверки без исполнения (решение 134).
//! Кто вызывает: mod.rs (trade из контракта).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, num.rs, ctx.rs, symbols.rs.
//! Настройки: venue.order_time_in_force, venue.order_test_first [binance]; сроки итога — order_state.rs;
//! venue.my_trades_limit — fills.rs (все проверяются до отправки).
//!
//! Официально: POST /api/v3/order (вес 1, «Unfilled order count: 1») — ответ FULL: orderId, status, origQty,
//! executedQty, cummulativeQuoteQty, fills[] (price, qty, commission, commissionAsset); 5XX и −1007 TIMEOUT — «execution
//! status unknown» → поиск по origClientOrderId (order_state.rs::recover).
//! Решение 263 (Тигран 13.11): expiryReason — условное поле ответа ордера «When an order has expired» (rest-api.md
//! «Conditional fields in Order Responses», enums.md «Expiry Reasons»; CHANGELOG 2026-05-06 «Order query responses may
//! include an expiryReason field») — в журнал venue.order (expiry_note); логика исполнения не меняется.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, пробный ордер, отказ Binance),
/// InsufficientBalance, OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    place(ctx, req, p).await
}

/// Что делает: постановка уже подготовленного ордера и его итог (вынесено из trade без изменений — решение 268: тот же
/// путь у рыночной продажи, market_sell.rs). Принимает: окружение, запрос ядра, ордер после правил (параметры запроса).
/// Возвращает и ошибки: как trade.
pub(super) async fn place(
    ctx: &Ctx,
    req: TradeRequest,
    p: order_rules::Prepared,
) -> GatewayResult<TradeResult> {
    let placed = rest::signed(ctx, Method::POST, "/api/v3/order", p.params, Op::Trade).await;
    // HTTP успешен, а orderId нет — итог неизвестен (повтор дал бы второй ордер).
    let placed = placed.and_then(|r| match num::text_opt(&r, "orderId") {
        Some(id) => Ok((id, r)),
        None => Err(ctx
            .tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без orderId в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())),
    });
    let (order_id, reply) = match placed {
        Ok((id, r)) => (id, Some(r)),
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.client_id).await {
                Some(id) => (id, None),
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = match reply {
        Some(r) if order_state::is_final(&r) => r,
        _ => order_state::settle(ctx, &p.symbol, &order_id).await?,
    };
    if let Some(m) = expiry_note(&p.symbol, &order_id, &state) {
        ctx.log(Level::Info, "venue.order", m).await;
    }
    order_state::result(ctx, &p.symbol, req, &state, order_id, p.qty).await
}

/// Что делает: решение 263 — строка журнала о причине истечения ордера по полю expiryReason (enums.md «Expiry
/// Reasons»: UNFILLED_IOC_QUANTITY_EXPIRED, INSUFFICIENT_LIQUIDITY, EXECUTION_RULE_PRICE_RANGE_EXCEEDED и др.; NONE —
/// «The order has not expired»). Значение — как прислал Binance. Принимает: символ, orderId, ответ ордера.
/// Возвращает: текст; None — поля нет, оно пустое или NONE. Ошибок нет.
pub fn expiry_note(symbol: &str, order_id: &str, s: &Value) -> Option<String> {
    let why = s.get("expiryReason").and_then(Value::as_str)?.trim();
    if why.is_empty() || why == "NONE" {
        return None;
    }
    let st = s.get("status").and_then(Value::as_str).unwrap_or_default();
    let q = |f: &str| s.get(f).and_then(Value::as_str).unwrap_or("?").to_string();
    Some(format!(
        "Binance {symbol}: ордер {order_id} ({st}) не исполнился до конца, expiryReason {why} (исполнено {} из {})",
        q("executedQty"),
        q("origQty")
    ))
}
