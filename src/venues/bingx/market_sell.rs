//! Файл: venues/bingx/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на BingX (контракт trade_market_sell): ордер type =
//! MARKET на количество базовой монеты. Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона»
//! (код 100440 → PriceOutOfRange), когда по свежему стакану продажа выгодна (core/sell_range.rs).
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: сторона через API, округление количества до
//! шага, minQty / maxQty, minNotional / maxNotional; сумма оценивается по худшей цене запроса — в ордер цена не идёт).
//! Постановка и итог — тем же кодом, что у лимитного (trade::place → order_state.rs: исполнено — по исполненному
//! количеству и сумме, цена запроса не участвует). Ошибка рыночного ордера никогда не несёт PriceOutOfRange.
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (place), symbols.rs, ctx.rs.
//! Настройки: ожидание итога — order_state.rs.
//!
//! Официальная документация: Place order, POST /openApi/spot/v1/trade/order (ссылки — trade.rs, order_rules.rs) —
//! параметры по постановке решения 268 (из этой страницы): type = MARKET, quantity — количество базовой монеты;
//! timeInForce — только для LIMIT (не передаётся); newClientOrderId — как у лимитного ордера.

use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::symbols::Symbols;

/// Что делает: параметры POST /openApi/spot/v1/trade/order рыночной продажи. Принимает: символ, количество базовой
/// монеты (после округления), свой ID ордера. Возвращает: параметры запроса. Ошибок нет.
pub fn params(symbol: &str, qty: Decimal, client_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("symbol", symbol.to_string()),
        ("side", "SELL".to_string()),
        ("type", "MARKET".to_string()),
        ("quantity", qty.normalize().to_string()),
        ("newClientOrderId", client_id.to_string()),
    ]
}

/// Что делает: рыночный ордер продажи после правил пары (без сети). Принимает: окружение, справочник пар (свежесть — у
/// вызывающего), запрос. Возвращает: Prepared (price — худшая цена запроса после округления, только для оценки суммы).
/// Ошибки: NotApplicable (не продажа), OrderRejected (правила), BadResponse (пары нет в справочнике).
pub fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    if req.side != Side::Sell {
        let m = "рыночным ордером — только продажа (решение 268)";
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
    }
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let (qty, price) = order_rules::check_rules(&rules, req.side, req.qty, req.worst_price)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    let client_id = order_rules::client_id(false, &req.deal_id);
    let params = params(&rules.symbol, qty, &client_id);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        params,
        client_id,
    })
}

/// Что делает: рыночная продажа — правила, постановка и итог тем же кодом, что у лимитного ордера (trade::place).
/// Принимает: окружение, справочник пар, запрос (сторона Sell). Возвращает: факт. Ошибки: как у trade, но без
/// PriceOutOfRange (заменяется на OrderRejected); NotApplicable — не продажа.
pub async fn sell(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = ready(ctx, symbols, &req)?;
    super::trade::place(ctx, req, p)
        .await
        .map_err(GatewayError::no_price_range)
}
