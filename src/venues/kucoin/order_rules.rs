//! Файл: venues/kucoin/order_rules.rs
//! Что делает: правила ордера KuCoin — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется, срок действия снимает неисполненное (IOC/FOK), количество и цена
//! после округления до шагов, минимум и максимум количества, минимальная сумма, сроки ожидания итога; если включено
//! venue.order_test_first — пробный ордер POST /api/v1/hf/orders/test с тем же телом.
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), тесты.
//! От чего зависит: symbols.rs (правила пары), rest.rs (пробный ордер), order_state.rs (сроки), ctx.rs.
//! Настройки: venue.order_time_in_force [kucoin] (IOC или FOK), venue.order_test_first [kucoin]; через order_state.rs —
//! venue.order_poll_ms, venue.order_final_wait_ms (без них ордер не ставится: после постановки — поздно).
//!
//! Официально:
//! - Add Order (HF) https://www.kucoin.com/docs-new/api-3470188 и Add Order Sync https://www.kucoin.com/docs-new/api-3470170
//!   — clientOid («can only contain numbers, letters, underscores (_), and hyphens (-)… The maximum length for the
//!   ClientOid is 40 characters», «Please do not initiate requests using the same clientOid»), side buy/sell, symbol,
//!   type limit, price («must be a multiple of priceIncrement»), size («a positive-integer multiple of baseIncrement and
//!   must be between baseMinSize and baseMaxSize»), timeInForce GTC/GTT/IOC/FOK.
//! - Add Order Test https://www.kucoin.com/docs-new/api-3470187 — «the request parameters and return parameters of this
//!   endpoint are exactly the same as the order endpoint… the order will not enter the matching system».
//! - Защита цены priceLimitRate (GET /api/v2/symbols) правилами не проверяется: последней цены у адаптера нет — её
//!   ловит пробный ордер или отказ биржи.

use reqwest::Method;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::symbols::{SymbolRules, Symbols};
use super::{order_state, rest};

/// Предел длины clientOid — официально «The maximum length for the ClientOid is 40 characters».
const MAX_CLIENT_OID: usize = 40;
/// Сроки действия, при которых неисполненное снимает биржа (правило Тиграна): IOC и FOK из перечня timeInForce.
const TIF_ALLOWED: [&str; 2] = ["IOC", "FOK"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub client_oid: String,
    /// Тело запроса (то же ушло в пробный ордер).
    pub body: Value,
}

/// Что делает: правила пары без сети — торгуется ли, срок действия, округление, пределы.
/// Принимает: правила пары, сторону, количество, худшую цену, срок действия (venue.order_time_in_force).
/// Возвращает: (количество, цена) после округления. Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    rules: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
    tif: &str,
) -> Result<(Decimal, Decimal), String> {
    let s = &rules.symbol;
    if !rules.tradable() {
        return Err(format!(
            "{s}: пара не торгуется (enableTrading или аукцион открытия), статус {}",
            rules.status
        ));
    }
    if !TIF_ALLOWED.contains(&tif) {
        return Err(format!(
            "{s}: venue.order_time_in_force = {tif} — нужен IOC или FOK (неисполненное снимает биржа)"
        ));
    }
    let qty = round(qty, rules.step_size, RoundingStrategy::ToZero);
    let how = if side == Side::Buy {
        RoundingStrategy::ToZero
    } else {
        RoundingStrategy::AwayFromZero
    };
    let price = round(worst, rules.tick_size, how);
    if qty <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(format!(
            "{s}: после округления количество {qty} или цена {price} не больше нуля"
        ));
    }
    if let Some(min) = rules.min_qty
        && qty < min
    {
        return Err(format!("{s}: количество {qty} меньше baseMinSize {min}"));
    }
    if let Some(max) = rules.max_qty
        && qty > max
    {
        return Err(format!("{s}: количество {qty} больше baseMaxSize {max}"));
    }
    let notional = qty * price;
    if let Some(min) = rules.min_notional
        && notional < min
    {
        return Err(format!("{s}: сумма {notional} меньше minFunds {min}"));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки — правила пары, сроки ожидания, пробный ордер. Вызывают И
/// настоящий ордер (trade.rs), И проверка без исполнения (check_order).
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила, отказ пробного), NotConfigured (настройки), BadResponse (пары нет), сбой пробного.
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| reject(ctx, m))?;
    order_state::times(ctx)?;
    let buy = req.side == Side::Buy;
    let client_oid = client_oid(buy, &req.deal_id);
    let body = json!({
        "clientOid": client_oid,
        "side": if buy { "buy" } else { "sell" },
        "symbol": rules.symbol,
        "type": "limit",
        "price": price.normalize().to_string(),
        "size": qty.normalize().to_string(),
        "timeInForce": tif,
    });
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        test(ctx, &rules.symbol, body.clone()).await?;
    }
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_oid,
        body,
    })
}

/// Пробный ордер: проверка параметров без исполнения. Код ошибки сохраняется (RateLimited, Unreachable и др.);
/// непонятный ответ — OrderRejected.
async fn test(ctx: &Ctx, symbol: &str, body: Value) -> GatewayResult<()> {
    let path = "/api/v1/hf/orders/test";
    rest::signed(
        ctx,
        Method::POST,
        path,
        vec![],
        Some(body),
        Op::Other,
        Pool::Spot,
    )
    .await
    .map(|_| ())
    .map_err(|e| {
        let code = match e.code {
            ErrorCode::BadResponse => ErrorCode::OrderRejected,
            c => c,
        };
        let message = format!("{symbol}: пробный ордер отклонён — {}", e.message);
        ctx.tag(GatewayError { code, message, ..e })
    })
}

/// Свой ID ордера (решение 114): новый у каждой попытки — метка (мс, 9 цифр) в начале, номер сделки — для поиска.
/// Возвращает: буквы, цифры, «_», «-», не длиннее 40. Ошибок нет.
pub fn client_oid(buy: bool, deal_id: &str) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    format!("{}{attempt:09}_{deal_id}", if buy { "b" } else { "s" })
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(MAX_CLIENT_OID)
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
