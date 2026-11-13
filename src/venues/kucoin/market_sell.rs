//! Файл: venues/kucoin/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на KuCoin (контракт trade_market_sell): ордер type = market на количество базовой монеты (size).
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус пары, округление количества до шага, минимумы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::place → order_state.rs: исполнено — по dealSize и dealFunds; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (place), order_state.rs, symbols.rs, ctx.rs.
//! Настройки: venue.order_time_in_force (только для общих правил пары), ожидание итога — order_state.rs.
//!
//! Официальная документация: Add Order Sync POST /api/v1/hf/orders/sync (ссылка — trade.rs; тело —
//! как у POST /api/v1/hf/orders) — параметры по постановке решения 268 (из официальной документации): type = market,
//! size — количество базовой монеты; price и timeInForce не передаются. Рыночный ордер KuCoin исполняется частично до
//! порога защиты цены — остаток обрабатывается как частичное исполнение.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::Symbols;

/// Что делает: тело Add Order рыночной продажи. Принимает: символ, количество базовой монеты (после округления),
/// свой clientOid. Возвращает: тело запроса. Ошибок нет.
pub fn body(symbol: &str, qty: Decimal, client_oid: &str) -> Value {
    json!({
        "clientOid": client_oid,
        "side": "sell",
        "symbol": symbol,
        "type": "market",
        "size": qty.normalize().to_string(),
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
    let client_oid = order_rules::client_oid(false, &req.deal_id);
    let body = body(&rules.symbol, qty, &client_oid);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_oid,
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
