//! Файл: venues/bybit/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на Bybit (контракт trade_market_sell): ордер orderType = Market, side = Sell на количество базовой монеты (qty, marketUnit = baseCoin).
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус пары, округление количества до шага, пределы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::trade → trade::result: исполнено — по cumExecQty и cumExecValue; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (trade), order_state.rs, symbols.rs, ctx.rs.
//! Настройки: ожидание итога — order_state.rs, venue.my_trades_limit — fills.rs.
//!
//! Официальная документация: «Place Order» POST /v5/order/create
//! (https://bybit-exchange.github.io/docs/v5/order/create-order) — параметры по постановке решения 268 (из этой
//! страницы): orderType = Market, side = Sell, qty — количество базовой монеты, marketUnit = baseCoin; price и
//! timeInForce не передаются. Проверка коридора venue.price_limit_check (лимитная цена) у рыночного ордера не делается.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::Symbols;

/// Что делает: тело Place Order рыночной продажи. Принимает: символ, количество базовой монеты (после округления),
/// свой orderLinkId. Возвращает: тело запроса. Ошибок нет.
pub fn body(symbol: &str, qty: Decimal, link_id: &str) -> Value {
    json!({
        "category": "spot",
        "symbol": symbol,
        "isLeverage": 0,
        "side": "Sell",
        "orderType": "Market",
        "qty": qty.normalize().to_string(),
        "marketUnit": "baseCoin",
        "orderLinkId": link_id,
    })
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
    let (qty, price) = order_rules::check_rules(&rules, req.side, req.qty, req.worst_price)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    super::fills::limit(ctx)?;
    let link_id = order_rules::link_id(false, &req.deal_id);
    let body = body(&rules.symbol, qty, &link_id);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        link_id,
        body,
    })
}

/// Что делает: рыночная продажа — правила, постановка и итог тем же кодом, что у лимитного ордера (trade::trade с готовым ордером).
/// Принимает: окружение, справочник пар, запрос (сторона Sell). Возвращает: факт. Ошибки: как у trade, но без
/// PriceOutOfRange (заменяется на OrderRejected); NotApplicable — см. ready.
pub async fn sell(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = ready(ctx, symbols, &req)?;
    super::trade::trade(ctx, symbols, req, Some(p))
        .await
        .map_err(GatewayError::no_price_range)
}
