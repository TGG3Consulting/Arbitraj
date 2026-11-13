//! Файл: venues/kraken/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на Kraken (контракт trade_market_sell): ордер ordertype = market, type = sell на количество базовой монеты (volume).
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус пары, округление количества, минимумы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::trade → order_state.rs: исполнено — по vol_exec и средней цене price ответа; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (trade), order_state.rs, symbols.rs, ctx.rs.
//! Настройки: venue.order_time_in_force (только для общих правил пары), ожидание итога — order_state.rs.
//!
//! Официальная документация: POST /0/private/AddOrder (OpenAPI spot-rest.yaml, схема add — ссылка в
//! trade.rs) — параметры по постановке решения 268 (из официальной документации): ordertype = market, type = sell,
//! volume — количество базовой монеты; price и timeinforce не передаются. oflags = fciq — как у лимитного ордера
//! (комиссия в котируемой монете: по ней order_state.rs считает комиссию).

use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::rest::{Params, p};
use super::symbols::Symbols;

/// Что делает: форма AddOrder рыночной продажи. Принимает: пару, количество базовой монеты (после округления),
/// свой cl_ord_id. Возвращает: поля формы. Ошибок нет.
pub fn form(symbol: &str, qty: Decimal, cl_ord_id: &str) -> Params {
    vec![
        p("ordertype", "market"),
        p("type", "sell"),
        p("pair", symbol.to_string()),
        p("volume", qty.normalize().to_string()),
        p("oflags", order_rules::FEE_IN_QUOTE),
        p("cl_ord_id", cl_ord_id.to_string()),
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
    let cl_ord_id = order_rules::cl_ord_id(false, &req.deal_id);
    let form = form(&rules.symbol, qty, &cl_ord_id);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        cl_ord_id,
        form,
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
