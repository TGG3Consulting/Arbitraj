//! Файл: venues/mexc/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на MEXC (контракт trade_market_sell): ордер type =
//! MARKET на количество базовой монеты, без price. Ядро зовёт только после отказа лимитного ордера продажи «цена вне
//! диапазона» (код 30087 → PriceOutOfRange), когда по свежему стакану продажа выгодна (core/sell_range.rs).
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: сторона через API, округление количества до
//! шага, минимумы и максимум; сумма оценивается по худшей цене запроса — в ордер цена не идёт); тип MARKET должен быть
//! в orderTypes пары — иначе NotApplicable с причиной. Постановка и итог — тем же кодом, что у лимитного ордера
//! (trade::trade → order_state.rs: исполнено — по executedQty и cumulativeQuoteQty, цена запроса не участвует).
//! Пробный ордер (venue.order_test_first) не делается. Ошибка рыночного ордера никогда не несёт PriceOutOfRange.
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (trade), order_state.rs, fills.rs, symbols.rs, ctx.rs.
//! Настройки: ожидание итога — order_state.rs, venue.my_trades_limit — fills.rs (проверяются до отправки).
//!
//! Официальная документация:
//! - POST /api/v3/order https://www.mexc.com/api-docs/spot-v3/spot-account-trade/new-order — параметры по постановке
//!   решения 268 (из этой страницы): type = MARKET, quantity — количество базовой монеты; price и timeInForce не
//!   передаются.
//! - orderTypes пары — GET /api/v3/exchangeInfo
//!   https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information.
//! - Отказы 30018 «market order is disabled», 30019 «api market order is disabled», 30041 — обычный OrderRejected.
//!
//! Не подтверждено живым ордером: применяет ли MEXC коридор PERCENT_PRICE_BY_SIDE к MARKET SELL (docs/TASKS.md).

use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Тип рыночного ордера (public-api-definitions: MARKET).
pub const MARKET: &str = "MARKET";

/// Что делает: параметры POST /api/v3/order рыночной продажи. Принимает: символ, количество базовой монеты (после
/// округления), свой ID ордера. Возвращает: параметры запроса. Ошибок нет.
pub fn params(symbol: &str, qty: Decimal, client_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("symbol", symbol.to_string()),
        ("side", "SELL".to_string()),
        ("type", MARKET.to_string()),
        ("quantity", qty.normalize().to_string()),
        ("newClientOrderId", client_id.to_string()),
    ]
}

/// Что делает: доступна ли рыночная продажа на паре — MARKET в orderTypes (поля нет — не ограничивает). Принимает:
/// правила пары. Возвращает: None — доступна; Some — причина. Ошибок нет.
pub fn market_block(rules: &SymbolRules) -> Option<String> {
    let types = rules.order_types.as_ref()?;
    (!rules.order_type_allowed(MARKET)).then(|| {
        format!(
            "{}: типа ордера MARKET нет в orderTypes пары ({}) — рыночная продажа недоступна",
            rules.symbol,
            types.join(", ")
        )
    })
}

/// Что делает: рыночный ордер продажи после правил пары (без сети). Принимает: окружение, справочник пар (свежесть — у
/// вызывающего), запрос. Возвращает: Prepared (price — худшая цена запроса после округления, только для оценки суммы).
/// Ошибки: NotApplicable (не продажа; MARKET нет в orderTypes), OrderRejected (правила), NotConfigured, BadResponse.
pub fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    if req.side != Side::Sell {
        let m = "рыночным ордером — только продажа (решение 268)";
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
    }
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    if let Some(why) = market_block(&rules) {
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, why)));
    }
    let (qty, price) = order_rules::check_rules(&rules, req.side, req.qty, req.worst_price, MARKET)
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
/// PriceOutOfRange (заменяется на OrderRejected); NotApplicable — см. ready.
pub async fn sell(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = ready(ctx, symbols, &req)?;
    super::trade::trade(ctx, symbols, req, Some(p))
        .await
        .map_err(GatewayError::no_price_range)
}
