//! Файл: venues/mexc/trade.rs
//! Что делает: покупка или продажа на MEXC ордером IMMEDIATE_OR_CANCEL с худшей допустимой ценой (что не исполнилось
//! сразу — снимает биржа) и проверкой факта (количество, средняя цена, комиссия — order_state.rs).
//! Правила пары, округление, пределы и пробный ордер (POST /api/v3/order/test, venue.order_test_first [mexc]) —
//! order_rules.rs: та же функция у проверки ордера без исполнения перед покупкой (решение 134).
//! Кто вызывает: mod.rs (trade из контракта).
//! От чего зависит: order_rules.rs (правила и пробный ордер), rest.rs, symbols.rs, num.rs, ctx.rs, order_state.rs.
//! Настройки: venue.order_type [mexc] (в orderTypes пары; IOC/FOK — при LIMIT, решение 133), venue.order_test_first [mexc];
//! ожидание итога — order_state.rs, venue.my_trades_limit — fills.rs (проверяются до отправки).
//!
//! Официальная документация:
//! - POST /api/v3/order https://www.mexc.com/api-docs/spot-v3/spot-account-trade/new-order — symbol, side, type,
//!   quantity, price, newClientOrderId; «Rate limit: Shared with the cancel order endpoint, 12 requests/second»;
//!   ответ — orderId без статуса и исполнения (итог — запросом ордера).
//! - Типы ордера https://www.mexc.com/api-docs/spot-v3/public-api-definitions — LIMIT, MARKET, LIMIT_MAKER,
//!   IMMEDIATE_OR_CANCEL, FILL_OR_KILL (параметра timeInForce нет: IOC — это type).
//! - POST /api/v3/order/test — «Test order requests without submitting them to the matching engine», ответ {}.
//!
//! Фильтр PERCENT_PRICE_BY_SIDE (exchangeInfo): лимитная цена не дальше заданного процента от последней сделки —
//! последней цены у нас нет; нарушение — отказ MEXC (OrderRejected), деньги не двигаются.

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Правила, настройки и пробный ордер — order_rules::ready (та же
/// функция, что у проверки ордера без исполнения перед покупкой, решение 134). Ошибки: OrderRejected (пара не
/// торгуется, сумма вне пределов, отказ пробного ордера, отказ MEXC), OutcomeUnknown — ордер мог быть принят, итог не
/// получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let placed = rest::signed(ctx, Method::POST, "/api/v3/order", p.params, Op::Trade).await;
    // Решение 118, А-🟠3: HTTP успешен, а orderId нет — итог неизвестен (не обычный сбой: повтор дал бы второй ордер).
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
    let order_id = match placed {
        Ok(id) => id,
        // Итог неизвестен — поиск по своему ID; не найден — отмена по нему и снова поиск (А-🟠4).
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.client_id).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &p.symbol, &order_id).await?;
    order_state::result(ctx, &p.symbol, req, &state, order_id, p.qty).await
}
