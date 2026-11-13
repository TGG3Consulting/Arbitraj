//! Файл: venues/phemex/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на Phemex (контракт trade_market_sell): ордер ordType = Market, side = Sell на количество базовой монеты (qtyType = ByBase, baseQtyEv).
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус пары, округление и перевод количества в масштаб, пределы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::trade → order_state.rs: исполнено — по cumBaseValueEv и cumQuoteValueEv; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (trade), order_state.rs, symbols.rs, ctx.rs.
//! Настройки: venue.order_time_in_force (только для общих правил пары), ожидание итога — order_state.rs.
//!
//! Официальная документация: Place order (HTTP PUT, prefered) PUT /spot/orders/create
//! (https://phemex-docs.github.io/#place-order-http-put-prefered; сверено 13.11.2026) — параметры по постановке решения
//! 268 и таблице этой страницы: ordType — Market (возможные значения: Market, Limit, …), side — Sell, qtyType —
//! ByBase, baseQtyEv — «Required if qtyType = ByBase»; timeInForce — необязателен («No») — не передаётся; priceEp в
//! таблице без отметки обязательности — у рыночного ордера не передаётся (живым ордером не подтверждено,
//! docs/TASKS.md).

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::rest::Params;
use super::symbols::Symbols;

/// Что делает: параметры PUT /spot/orders/create рыночной продажи. Принимает: символ, свой clOrdID, количество
/// базовой монеты в масштабе (baseQtyEv). Возвращает: параметры запроса. Ошибок нет.
pub fn query(symbol: &str, cl_ord_id: &str, qty_ev: i64) -> Params {
    vec![
        ("symbol", symbol.to_string()),
        ("clOrdID", cl_ord_id.to_string()),
        ("side", "Sell".to_string()),
        ("qtyType", "ByBase".to_string()),
        ("baseQtyEv", qty_ev.to_string()),
        ("ordType", "Market".to_string()),
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
    let r = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    let x = order_rules::check_rules(&r, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let cl_ord_id = order_rules::uuid4().map_err(|e| ctx.tag(e))?;
    let query = query(&r.symbol, &cl_ord_id, x.qty_ev);
    Ok(Prepared {
        symbol: r.symbol,
        qty: x.qty,
        price: x.price,
        cl_ord_id,
        received: r.quote,
        base_scale: r.base_scale,
        quote_scale: r.quote_scale,
        query,
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
