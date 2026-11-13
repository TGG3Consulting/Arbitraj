//! Файл: venues/xt/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на XT (контракт trade_market_sell): ордер type = MARKET, side = SELL на количество базовой монеты (quantity), timeInForce = IOC.
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules с timeInForce IOC: статус и стороны API, IOC в timeInForces пары, округление количества, пределы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::place → order_state.rs: исполнено — по исполненному количеству и средней цене ответа; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Тип MARKET должен быть в orderTypes пары (поле есть и MARKET в нём нет — NotApplicable с причиной).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (place), order_state.rs, symbols.rs, ctx.rs.
//! Настройки: ожидание итога — order_state.rs.
//!
//! Официальная документация: POST /v4/order (https://doc.xt.com/docs/spot/Order/SubmitOrder) —
//! параметры по постановке решения 268 (из этой страницы): type = MARKET, side = SELL, quantity — количество базовой
//! монеты, quoteQty не передаётся, bizType = SPOT, timeInForce обязателен — IOC. Отказ ORDER_F0601 «Trigger
//! PROTECTION_MARKET Filter» — обычный OrderRejected. orderTypes пары —
//! https://doc.xt.com/docs/spot/Market/GetSymbolInformation.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// timeInForce рыночного ордера (обязателен — постановка решения 268).
pub const TIF: &str = "IOC";

/// Что делает: тело POST /v4/order рыночной продажи. Принимает: символ, количество базовой монеты (после
/// округления), свой clientOrderId. Возвращает: тело запроса. Ошибок нет.
pub fn body(symbol: &str, qty: Decimal, client_id: &str) -> Value {
    json!({
        "symbol": symbol,
        "side": "SELL",
        "type": "MARKET",
        "timeInForce": TIF,
        "bizType": "SPOT",
        "quantity": order_rules::number(qty),
        "clientOrderId": client_id,
    })
}

/// Что делает: доступна ли рыночная продажа на паре — MARKET в orderTypes (поля нет — не ограничивает; пустой список —
/// значения нет, как у order_rules::order_block). Принимает: правила пары. Возвращает: None — доступна; Some — причина.
pub fn market_block(r: &SymbolRules) -> Option<String> {
    let types = r.order_types.as_ref()?;
    (!types.iter().any(|t| t == "MARKET")).then(|| {
        format!(
            "{}: типа MARKET нет в orderTypes пары ({}) — рыночная продажа недоступна",
            r.symbol,
            types.join(", ")
        )
    })
}

/// Что делает: рыночный ордер продажи после правил пары (без сети). Принимает: окружение, справочник пар (свежесть — у
/// вызывающего), запрос. Возвращает: Prepared (price — худшая цена запроса после округления, только для оценки суммы).
/// Ошибки: NotApplicable (не продажа; MARKET нет в orderTypes), OrderRejected (правила), NotConfigured, BadResponse (пары нет).
pub fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    if req.side != Side::Sell {
        let m = "рыночным ордером — только продажа (решение 268)";
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
    }
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    if let Some(why) = market_block(&rules) {
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, why)));
    }
    let (qty, price) = order_rules::check_rules(&rules, req.side, req.qty, req.worst_price, TIF)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let client_id = order_rules::client_id(false);
    let body = body(&rules.symbol, qty, &client_id);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_id,
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
