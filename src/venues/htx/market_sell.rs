//! Файл: venues/htx/market_sell.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — РЫНОЧНАЯ ПРОДАЖА на HTX (контракт trade_market_sell): ордер type = sell-market на количество базовой монеты (amount), без price.
//! Ядро зовёт только после отказа лимитного ордера продажи «цена вне диапазона» (PriceOutOfRange), когда по свежему
//! стакану продажа выгодна (core/sell_range.rs). Покупок рыночным ордером нет.
//! Правила пары — те же, что у лимитного ордера (order_rules::check_rules: статус пары, округление количества, пределы; сумма оценивается по худшей цене запроса — в ордер цена не
//! идёт). Постановка и итог — тем же кодом, что у лимитного (trade::trade → order_state.rs: исполнено — по field-amount и field-cash-amount; цена запроса в разборе итога не
//! участвует; исполнено меньше запрошенного — частичное исполнение). Пробный ордер не делается. Ошибка рыночного
//! ордера никогда не несёт PriceOutOfRange (заменяется на OrderRejected).
//! Кто вызывает: mod.rs (trade_market_sell из контракта).
//! От чего зависит: order_rules.rs, trade.rs (trade), order_state.rs, symbols.rs, accounts.rs (спот-счёт), ctx.rs.
//! Настройки: ожидание итога — order_state.rs.
//!
//! Официальная документация: Place a New Order POST /v1/order/orders/place
//! (https://huobiapi.github.io/docs/spot/v1/en/#place-a-new-order) — параметры по постановке решения 268 (из этой
//! страницы): type = sell-market, amount — количество базовой монеты; price не передаётся.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::accounts::SpotId;
use super::ctx::Ctx;
use super::order_rules::{self, Prepared};
use super::order_state;
use super::symbols::Symbols;

/// Тип рыночного ордера продажи (постановка решения 268).
pub const KIND: &str = "sell-market";

/// Что делает: тело Place a New Order рыночной продажи. Принимает: id спот-счёта, символ, количество базовой монеты
/// (после округления), свой client-order-id. Возвращает: тело запроса. Ошибок нет.
pub fn body(account: i64, symbol: &str, qty: Decimal, client_oid: &str) -> Value {
    json!({
        "account-id": account.to_string(),
        "symbol": symbol,
        "type": KIND,
        "amount": qty.normalize().to_string(),
        "source": order_rules::SOURCE,
        "client-order-id": client_oid,
    })
}

/// Что делает: рыночный ордер продажи после правил пары (сеть — только id спот-счёта, как у лимитного ордера).
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), спот-счёт, запрос. Возвращает: Prepared (price —
/// худшая цена запроса после округления, только для оценки суммы).
/// Ошибки: NotApplicable (не продажа), OrderRejected (правила), NotConfigured, BadResponse (пары нет).
pub async fn ready(
    ctx: &Ctx,
    symbols: &Symbols,
    spot: &SpotId,
    req: &TradeRequest,
) -> GatewayResult<Prepared> {
    if req.side != Side::Sell {
        let m = "рыночным ордером — только продажа (решение 268)";
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
    }
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let (qty, price) = order_rules::check_rules(&rules, req.side, req.qty, req.worst_price)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let account = spot.require(ctx).await?;
    let client_oid = order_rules::client_oid('s', &req.deal_id, order_rules::MAX_ORDER_CID);
    let body = body(account, &rules.symbol, qty, &client_oid);
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_oid,
        received: rules.quote,
        body,
    })
}

/// Что делает: рыночная продажа — правила, постановка и итог тем же кодом, что у лимитного ордера (trade::trade с готовым ордером).
/// Принимает: окружение, справочник пар, спот-счёт, запрос (сторона Sell). Возвращает: факт. Ошибки: как у trade, но
/// без PriceOutOfRange (заменяется на OrderRejected); NotApplicable — не продажа.
pub async fn sell(
    ctx: &Ctx,
    symbols: &Symbols,
    spot: &SpotId,
    req: TradeRequest,
) -> GatewayResult<TradeResult> {
    let p = ready(ctx, symbols, spot, &req).await?;
    super::trade::trade(ctx, symbols, spot, req, Some(p))
        .await
        .map_err(GatewayError::no_price_range)
}
