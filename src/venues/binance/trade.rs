//! Файл: venues/binance/trade.rs
//! Что делает: покупка или продажа на Binance лимитным ордером IOC (или FOK) с худшей допустимой ценой — что не
//! исполнилось сразу, снимает биржа (статус EXPIRED). Итог берётся из ответа FULL (status, executedQty,
//! cummulativeQuoteQty, fills с комиссией); если ответ не окончательный — опрос и отмена (order_state.rs).
//! Правила пары — order_rules::ready: та же функция у проверки без исполнения (решение 134). Пробного ордера перед
//! настоящим (покупка и продажа) нет — решение 412: он только в проверке без исполнения (order_rules::check).
//! Кто вызывает: mod.rs (trade из контракта).
//! Расчётный круг Base (USDC ↔ USDT): в области gateway/settlement_intent.rs свой ID ордера сохраняется до отправки
//! (before_send), ошибка «итог неизвестен» несёт «чем найти» (PendingRef); сверка — settle.rs.
//! Решение 268: тот же путь постановки и итога — у рыночной продажи (market_sell.rs передаёт готовый ордер, prepared).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, num.rs, ctx.rs, symbols.rs.
//! Настройки: venue.order_time_in_force [binance]; сроки итога — order_state.rs;
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
use crate::gateway::pending::PendingRef;
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат (пробного ордера нет — решение 412). Ошибки: OrderRejected
/// (правила, отказ Binance),
/// InsufficientBalance, OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
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
    // Расчётный круг Base (контракт supports_settlement_order): свой ID ордера (newClientOrderId), символ и количество
    // сохраняются ядром ДО отправки (gateway/settlement_intent.rs before_send) — после потери ответа ордер ищется по ним
    // (settle.rs inspect, GET /api/v3/order по origClientOrderId). Вне расчётного круга — как раньше, без записи.
    let settling = crate::gateway::settlement_intent::active();
    let lookup = PendingRef::order(&p.symbol, &p.client_id, p.qty);
    if settling {
        crate::gateway::settlement_intent::before_send(&lookup).await?;
    }
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
                None => return Err(with_lookup(settling, e, &lookup)),
            }
        }
        Err(e) => return Err(e),
    };
    let state = match reply {
        Some(r) if order_state::is_final(&r) => r,
        _ => order_state::settle(ctx, &p.symbol, &order_id)
            .await
            .map_err(|e| with_lookup(settling, e, &lookup))?,
    };
    if let Some(m) = expiry_note(&p.symbol, &order_id, &state) {
        ctx.log(Level::Info, "venue.order", m).await;
    }
    order_state::result(ctx, &p.symbol, req, &state, order_id, p.qty).await
}

/// Что делает: в расчётном круге прикладывает к ошибке «итог неизвестен» (OutcomeUnknown) «чем найти» ордер —
/// символ, свой ID, количество (решение 231, gateway/pending.rs). Вне расчётного круга ошибка не меняется (обычные
/// сделки ведут себя как раньше). Принимает: признак расчётного круга, ошибку, «чем найти». Возвращает: ошибку.
/// Ошибок нет.
fn with_lookup(settling: bool, e: GatewayError, lookup: &PendingRef) -> GatewayError {
    if settling && e.code == ErrorCode::OutcomeUnknown && e.pending.is_none() {
        e.pending(lookup.clone())
    } else {
        e
    }
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
