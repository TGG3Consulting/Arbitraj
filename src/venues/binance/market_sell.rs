//! Файл: venues/binance/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на Binance (контракт trade_market_sell): ордер type =
//! MARKET на количество базовой монеты, ответ FULL — как у лимитного. Ядро зовёт только после отказа лимитного ордера
//! продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему стакану продажа выгодна (core/sell_range.rs).
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус и права, округление количества до
//! шага LOT_SIZE, пределы; сумма NOTIONAL оценивается по худшей цене запроса — в ордер цена не идёт). Фильтр
//! MARKET_LOT_SIZE адаптером не разбирается (symbols.rs) — нарушение даст отказ биржи; пара без MARKET в orderTypes —
//! отказ биржи как есть. Постановка и итог — тем же кодом, что у лимитного (trade::trade → order_state.rs: исполнено —
//! по executedQty и cummulativeQuoteQty, цена запроса не участвует). Пробный ордер не делается. Ошибка рыночного ордера
//! никогда не несёт PriceOutOfRange (и истечение EXECUTION_RULE_PRICE_RANGE_EXCEEDED — тоже: OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (trade), order_state.rs, fills.rs, symbols.rs, ctx.rs.
//! Настройки: venue.order_time_in_force (только для общих правил пары), ожидание итога — order_state.rs,
//! venue.my_trades_limit — fills.rs.
//!
//! Официальная документация: POST /api/v3/order (rest-api «Trading endpoints», ссылка — order_rules.rs) — параметры по
//! постановке решения 268 (из этой страницы): type = MARKET, quantity — количество базовой монеты, newOrderRespType
//! FULL, как у лимитного; price и timeInForce не передаются.

use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::Symbols;

/// Что делает: параметры POST /api/v3/order рыночной продажи. Принимает: символ, количество базовой монеты (после
/// округления), свой ID ордера. Возвращает: параметры запроса. Ошибок нет.
pub fn params(symbol: &str, qty: Decimal, client_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("symbol", symbol.to_string()),
        ("side", "SELL".to_string()),
        ("type", "MARKET".to_string()),
        ("quantity", qty.normalize().to_string()),
        ("newClientOrderId", client_id.to_string()),
        ("newOrderRespType", "FULL".to_string()),
    ]
}

/// Что делает: рыночный ордер продажи после правил пары (без сети). Принимает: окружение, справочник пар (свежесть — у
/// вызывающего), запрос. Возвращает: Prepared (price — худшая цена запроса после округления, только для оценки суммы).
/// Ошибки: NotApplicable (не продажа), OrderRejected (правила), NotConfigured, BadResponse (пары нет).
pub fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    if req.side != Side::Sell {
        let m = "рыночным ордером — только продажа (решение 268)";
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
    }
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) =
        order_rules::check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
            .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    super::fills::limit(ctx)?;
    let client_id = order_rules::client_id(false, &req.deal_id);
    let params = params(&rules.symbol, qty, &client_id);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_id,
        params,
    })
}

/// Что делает: рыночная продажа — правила, постановка и итог тем же кодом, что у лимитного ордера (trade::trade с готовым ордером).
/// Принимает: окружение, справочник пар, запрос (сторона Sell). Возвращает: факт. Ошибки: как у trade, но без
/// PriceOutOfRange (заменяется на OrderRejected); NotApplicable — не продажа.
pub async fn sell(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = ready(ctx, symbols, &req)?;
    super::trade::trade(ctx, symbols, req, Some(p))
        .await
        .map_err(GatewayError::no_price_range)
}
