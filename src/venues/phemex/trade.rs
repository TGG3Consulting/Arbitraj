//! Файл: venues/phemex/trade.rs
//! Что делает: покупка или продажа на Phemex лимитным ордером с худшей допустимой ценой и сроком ImmediateOrCancel /
//! FillOrKill (что не исполнилось сразу — снимает биржа); затем окончательное состояние и факт (количество, сумма,
//! комиссия и её монета — order_state.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Place order (HTTP PUT, prefered) PUT /spot/orders/create?… (https://phemex-docs.github.io/
//! #place-order-http-put-prefered), группа лимитов SpotOrder (500/мин): ответ data — ордер со статусом «Created» и
//! bizError; это приём ордера, не исполнение (итог — Query orders by client order ID). HTTP 5XX — «execution status is
//! UNKNOWN» (General API information): ордер ищется по своему clOrdID, не найден — отмена по clOrdID и снова поиск
//! (order_state::settle); не найден и тогда — «итог неизвестен» (решение 64). Повтора постановки нет.

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Phemex),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let path = "/spot/orders/create";
    let placed = rest::signed(ctx, Method::PUT, path, p.query.clone(), None, Op::Trade).await;
    match placed {
        Ok(_) => {}
        // Ордер мог быть принят — ищем по своему clOrdID (settle); не найден — OutcomeUnknown оттуда же.
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            let m = format!(
                "{}: {} — ищем ордер по clOrdID {}",
                p.symbol, e.message, p.cl_ord_id
            );
            ctx.log(Level::Warn, "venue.trade", m).await;
        }
        Err(e) => return Err(e),
    }
    let state = order_state::settle(ctx, &p).await?;
    order_state::result(ctx, req, &state, &p, &symbols.scales())
}
