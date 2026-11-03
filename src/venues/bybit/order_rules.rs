//! Файл: venues/bybit/order_rules.rs
//! Что делает: правила ордера Bybit — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется (Trading), срок действия venue.order_time_in_force — только IOC
//! или FOK (остаток снимает биржа — урок 5), количество вниз до basePrecision, цена до tickSize в безопасную сторону,
//! наибольшее количество maxLimitOrderQty, наименьшая сумма minOrderAmt, сроки ожидания итога и предел чтения
//! исполнений; при venue.price_limit_check — предел цены Bybit (GET /v5/market/price-limit).
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order).
//! От чего зависит: symbols.rs, rest.rs, num.rs, order_state.rs, fills.rs, ctx.rs.
//! Настройки: venue.order_time_in_force [bybit], venue.price_limit_check [bybit]; через order_state.rs и fills.rs —
//! venue.order_poll_ms, venue.order_final_wait_ms, venue.my_trades_limit (проверяются до отправки: после — поздно).
//!
//! Официальная документация:
//! - «Place Order» POST /v5/order/create https://bybit-exchange.github.io/docs/v5/order/create-order — category=spot,
//!   symbol, side (Buy, Sell), orderType Limit (Market не предлагается: правило «худшая цена»), qty (у Limit — в
//!   монете), price, timeInForce (GTC, IOC, FOK, PostOnly), isLeverage («0(default): false, spot trading»), orderLinkId
//!   («A max of 36 characters. Combinations of numbers, letters (upper and lower cases), dashes, and underscores»,
//!   «always unique»).
//! - Пробного ордера для спота нет: «Pre Check Order» POST /v5/order/pre-check — «supports orders with category =
//!   inverse, linear, option» (https://bybit-exchange.github.io/docs/v5/order/pre-check-order). check_order — только
//!   правила (и предел цены, если включён).
//! - «Get Order Price Limit» GET /v5/market/price-limit?category=spot&symbol= https://bybit-exchange.github.io/docs/v5/market/order-price-limit
//!   — buyLmt «Highest Bid Price», sellLmt «Lowest Ask Price». Цена покупки выше buyLmt или продажи ниже sellLmt — отказ.

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::symbols::{SymbolRules, Symbols};
use super::{num, order_state, rest};

/// Наибольшая длина orderLinkId — из официальной документации («A max of 36 characters»).
pub const MAX_LINK_ID: usize = 36;
/// Сроки действия, при которых неисполненный остаток снимает биржа (enum timeInForce: IOC, FOK).
const TIF_OK: [&str; 2] = ["IOC", "FOK"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    /// Свой ID ордера (orderLinkId) — для поиска при неизвестном итоге.
    pub link_id: String,
    /// Тело POST /v5/order/create.
    pub body: Value,
}

/// Что делает: правила пары без сети — статус, округление, пределы. Принимает: правила, сторону, количество, худшую
/// цену. Возвращает: (количество, цена) после округления. Ошибки: текст причины.
pub fn check_rules(
    r: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
) -> Result<(Decimal, Decimal), String> {
    let s = &r.symbol;
    if !r.tradable() {
        return Err(format!("{s}: пара не торгуется (status {})", r.status_text));
    }
    let buy = side == Side::Buy;
    let qty = round(qty, r.step_size, RoundingStrategy::ToZero);
    let how = if buy {
        RoundingStrategy::ToZero
    } else {
        RoundingStrategy::AwayFromZero
    };
    let price = round(worst, r.tick_size, how);
    if qty <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(format!(
            "{s}: после округления количество {qty} или цена {price} не больше нуля"
        ));
    }
    if let Some(max) = r.max_qty
        && qty > max
    {
        return Err(format!(
            "{s}: количество {qty} больше maxLimitOrderQty {max}"
        ));
    }
    if let Some(min) = r.min_notional
        && qty * price < min
    {
        return Err(format!(
            "{s}: сумма {} меньше minOrderAmt {min}",
            qty * price
        ));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки. Вызывают И настоящий ордер, И проверка без исполнения.
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила, предел цены), NotConfigured (настройки), BadResponse (пары нет), связь.
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    if !TIF_OK.contains(&tif.as_str()) {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.order_time_in_force",
            format!("venue.order_time_in_force = {tif}: у Bybit только IOC или FOK (остаток снимает биржа)"),
        )));
    }
    let (qty, price) =
        check_rules(&rules, req.side, req.qty, req.worst_price).map_err(|m| reject(ctx, m))?;
    order_state::times(ctx)?;
    super::fills::limit(ctx)?;
    let buy = req.side == Side::Buy;
    if ctx.flag("venue.price_limit_check").unwrap_or(false) {
        price_limit(ctx, &rules.symbol, buy, price).await?;
    }
    let link_id = link_id(buy, &req.deal_id);
    let body = json!({
        "category": "spot",
        "symbol": rules.symbol,
        "isLeverage": 0,
        "side": if buy { "Buy" } else { "Sell" },
        "orderType": "Limit",
        "qty": qty.normalize().to_string(),
        "price": price.normalize().to_string(),
        "timeInForce": tif,
        "orderLinkId": link_id,
    });
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        link_id,
        body,
    })
}

/// Предел цены Bybit: покупка не выше buyLmt, продажа не ниже sellLmt. Ошибки: OrderRejected, связь, формат.
async fn price_limit(ctx: &Ctx, symbol: &str, buy: bool, price: Decimal) -> GatewayResult<()> {
    let p = vec![
        ("category", "spot".to_string()),
        ("symbol", symbol.to_string()),
    ];
    let d = rest::public(ctx, "/v5/market/price-limit", p).await?;
    if buy {
        let lim = num::dec(&d, "buyLmt")?;
        if price > lim {
            return Err(reject(
                ctx,
                format!("{symbol}: цена покупки {price} выше предела Bybit buyLmt {lim}"),
            ));
        }
    } else {
        let lim = num::dec(&d, "sellLmt")?;
        if price < lim {
            return Err(reject(
                ctx,
                format!("{symbol}: цена продажи {price} ниже предела Bybit sellLmt {lim}"),
            ));
        }
    }
    Ok(())
}

/// Свой ID ордера (решение 114): свой у каждой попытки — метка попытки (мс, 9 цифр) в начале, номер сделки — для
/// поиска. Возвращает: буквы, цифры, «-», «_», не длиннее MAX_LINK_ID. Ошибок нет.
pub fn link_id(buy: bool, deal_id: &str) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    format!("{}{attempt:09}_{deal_id}", if buy { "b" } else { "s" })
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(MAX_LINK_ID)
        .collect()
}

/// Округление до шага (шаг 0 — без округления). Ошибок нет.
pub fn round(v: Decimal, step: Decimal, how: RoundingStrategy) -> Decimal {
    if step.is_zero() {
        return v;
    }
    ((v / step).round_dp_with_strategy(0, how) * step).normalize()
}

/// Ошибка «ордер не принят» до отправки.
fn reject(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, msg))
}
