//! Файл: venues/bitget/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на Bitget (контракт trade_market_sell): ордер orderType = market на количество базовой монеты (size).
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус пары, округление количества до шага, минимумы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::place → order_state.rs: исполнено — по baseVolume и quoteVolume; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (place), order_state.rs, symbols.rs, ctx.rs.
//! Настройки: venue.order_time_in_force (поле force), ожидание итога — order_state.rs.
//!
//! Официальная документация: Place Order POST /api/v2/spot/trade/place-order (ссылка — trade.rs) —
//! параметры по постановке решения 268 (из этой страницы): orderType = market, size — количество базовой монеты у
//! продажи; force для market недействителен, но в таблице обязателен — передаётся как у лимитного
//! (venue.order_time_in_force); price не передаётся.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::Symbols;

/// Что делает: тело Place Order рыночной продажи. Принимает: символ, количество базовой монеты (после округления),
/// force (как у лимитного ордера), свой clientOid. Возвращает: тело запроса. Ошибок нет.
pub fn body(symbol: &str, qty: Decimal, force: &str, client_oid: &str) -> Value {
    json!({
        "symbol": symbol,
        "side": "sell",
        "orderType": "market",
        "force": force,
        "size": qty.normalize().to_string(),
        "clientOid": client_oid,
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
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) =
        order_rules::check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
            .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let client_oid = order_rules::client_oid('s', &req.deal_id);
    let body = body(&rules.symbol, qty, &tif, &client_oid);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_oid,
        received: rules.quote,
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
