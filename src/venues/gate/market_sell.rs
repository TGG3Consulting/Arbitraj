//! Файл: venues/gate/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на Gate (контракт trade_market_sell): ордер type = market на количество базовой монеты (amount), time_in_force = ioc.
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус пары, округление количества до шага, минимумы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::place → order_state.rs: исполнено — по filled_amount и filled_total; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (place), order_state.rs, symbols.rs, ctx.rs.
//! Настройки: ожидание итога — order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms, venue.order_history_limit).
//!
//! Официальная документация: POST /spot/orders (ссылки — trade.rs, order_rules.rs) — параметры по
//! постановке решения 268 (из этой страницы): type = market, amount — количество базовой монеты у продажи,
//! time_in_force = ioc (для market допустимы только ioc и fok); price не передаётся.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::Symbols;

/// time_in_force рыночного ордера (для market допустимы только ioc и fok — постановка решения 268).
pub const TIF: &str = "ioc";

/// Что делает: тело POST /spot/orders рыночной продажи. Принимает: символ, количество базовой монеты (после
/// округления), свой text ордера. Возвращает: тело запроса. Ошибок нет.
pub fn body(symbol: &str, qty: Decimal, text: &str) -> Value {
    json!({
        "text": text,
        "currency_pair": symbol,
        "type": "market",
        "account": "spot",
        "side": "sell",
        "amount": qty.normalize().to_string(),
        "time_in_force": TIF,
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
    let (qty, price) = order_rules::check_rules(&rules, req.side, req.qty, req.worst_price, TIF)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    order_state::history_limit(ctx)?;
    let text = order_rules::text(false, &req.deal_id);
    let body = body(&rules.symbol, qty, &text);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        text,
        body,
    })
}

/// Что делает: рыночная продажа — правила, постановка и итог тем же кодом, что у лимитного ордера (trade::place).
/// Принимает: окружение, справочник пар, запрос (сторона Sell). Возвращает: факт. Ошибки: как у trade, но без
/// PriceOutOfRange (заменяется на OrderRejected); NotApplicable — см. ready.
pub async fn sell(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = ready(ctx, symbols, &req)?;
    super::trade::place(ctx, req, p)
        .await
        .map_err(GatewayError::no_price_range)
}
