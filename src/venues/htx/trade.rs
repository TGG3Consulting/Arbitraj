//! Файл: venues/htx/trade.rs
//! Что делает: покупка или продажа на HTX ордером buy-ioc / sell-ioc (или limit-fok) с худшей допустимой ценой — что не
//! исполнилось сразу, снимает биржа; затем окончательное состояние и факт (количество, средняя цена, комиссия и её
//! монета — order_state.rs, order_fee.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, accounts.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Place a New Order POST /v1/order/orders/place https://huobiapi.github.io/docs/spot/v1/en/#place-a-new-order:
//! ответ data — order-id строкой; это приём ордера, не исполнение (итог — деталь ордера). Ответ без order-id при
//! успешном HTTP — итог неизвестен (повтор дал бы второй ордер).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::accounts::SpotId;
use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ HTX),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(
    ctx: &Ctx,
    symbols: &Symbols,
    spot: &SpotId,
    req: TradeRequest,
) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, spot, &req).await?;
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
    let path = "/v1/order/orders/place";
    let placed = rest::signed(ctx, Method::POST, path, vec![], Some(p.body), Op::Trade).await;
    let placed = placed.and_then(|d| match d {
        Value::String(s) if !s.is_empty() => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(ctx
            .tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без order-id в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(other.to_string())),
    });
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.client_oid).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &order_id).await?;
    order_state::result(ctx, req, &state, order_id, &p.received).await
}
