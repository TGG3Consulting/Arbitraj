//! Файл: venues/poloniex/order_rules.rs
//! Что делает: правила ордера Poloniex — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется (state NORMAL), срок действия снимает неисполненное (IOC/FOK),
//! количество и цена после округления до шагов, минимумы и максимумы количества и суммы, границы цены, сроки ожидания
//! итога — всё до отправки. Пробного ордера у Poloniex нет: venue.order_test_first = да — ошибка настройки (как Gate).
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), transfers.rs
//! (client_id), тесты.
//! От чего зависит: symbols.rs (правила пары), order_state.rs (сроки), ctx.rs.
//! Настройки: venue.order_time_in_force [poloniex] (IOC или FOK), venue.order_test_first [poloniex] (только «нет»);
//! через order_state.rs — venue.order_poll_ms, venue.order_final_wait_ms (без них ордер не ставится: после — поздно).
//!
//! Официально:
//! - Create Order POST /orders https://api-docs.poloniex.com/spot/api/private/order — symbol, side BUY/SELL, type
//!   (MARKET по умолчанию — шлётся явно LIMIT), timeInForce GTC / IOC / FOK, price, quantity («base units»),
//!   clientOrderId, allowBorrow (false). Списка разрешённых timeInForce у пары нет (урок 30: IOC подтвердить живым
//!   ордером).
//! - clientOrderId: «Maximum 64 characters … A-Za-z0-9_-», а Error Codes 21315 — «Client orderId exceeded max length of
//!   17 digits» (противоречие, урок 2). Свой id — 17 цифр: проходит оба правила; новый на каждую попытку (урок 6).
//! - Error Codes: 21350 «Amount must be greater than 1 USDT»; 21310 / 21311 — цена выше / ниже границы; 21322 / 21330 —
//!   меньше minAmount / minQuantity. Ценовая защита 21356 («too much price movement») зависит от стакана — её
//!   проверяет биржа.

use std::sync::atomic::{AtomicU64, Ordering};

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Сроки действия, при которых неисполненное снимает биржа (правило Тиграна): из перечня timeInForce.
const TIF_ALLOWED: [&str; 2] = ["IOC", "FOK"];
/// Котируемая монета, к которой относится 21350 «Amount must be greater than 1 USDT».
const USDT: &str = "USDT";
/// Сумма ордера в USDT должна быть больше — Error Codes 21350.
const MIN_USDT_AMOUNT: Decimal = Decimal::ONE;
/// Длина своего id — 17 знаков (Error Codes 21315; меньше предела 64 из Create Order).
pub const CLIENT_ID_LEN: usize = 17;

/// Номер попытки внутри одной миллисекунды (своих id в одну мс — разные).
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub client_id: String,
    /// Монета, в которой приходит результат: покупка — базовая, продажа — котируемая.
    pub received: String,
    /// Тело запроса Create Order.
    pub body: Value,
}

/// Что делает: правила пары без сети — торгуется ли, срок действия, округление, пределы количества, суммы и цены.
/// Принимает: правила пары, сторону, количество, худшую цену, срок действия (venue.order_time_in_force).
/// Возвращает: (количество, цена) после округления. Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    r: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
    tif: &str,
) -> Result<(Decimal, Decimal), String> {
    let s = &r.symbol;
    if !r.tradable() {
        return Err(format!(
            "{s}: пара не торгуется (state не NORMAL), код {}",
            r.status
        ));
    }
    if !TIF_ALLOWED.contains(&tif) {
        return Err(format!(
            "{s}: venue.order_time_in_force = {tif} — нужен IOC или FOK (неисполненное снимает биржа)"
        ));
    }
    let qty = round(qty, r.step_size, RoundingStrategy::ToZero);
    let buy = side == Side::Buy;
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
    let amount = qty * price;
    let fails = [
        (
            r.min_qty.is_some_and(|m| qty < m),
            format!("количество {qty} меньше minQuantity {:?}", r.min_qty),
        ),
        (
            r.max_qty.is_some_and(|m| qty > m),
            format!("количество {qty} больше maxQuantity {:?}", r.max_qty),
        ),
        (
            r.min_notional.is_some_and(|m| amount < m),
            format!("сумма {amount} меньше minAmount {:?}", r.min_notional),
        ),
        (
            r.max_notional.is_some_and(|m| amount > m),
            format!("сумма {amount} больше maxAmount {:?}", r.max_notional),
        ),
        (
            r.quote == USDT && amount <= MIN_USDT_AMOUNT,
            format!("сумма {amount} USDT не больше 1 USDT (код 21350)"),
        ),
        (
            buy && r.highest_bid.is_some_and(|h| price > h),
            format!("цена покупки {price} выше highestBid {:?}", r.highest_bid),
        ),
        (
            !buy && r.lowest_ask.is_some_and(|l| price < l),
            format!("цена продажи {price} ниже lowestAsk {:?}", r.lowest_ask),
        ),
    ];
    if let Some((_, why)) = fails.into_iter().find(|(bad, _)| *bad) {
        return Err(format!("{s}: {why}"));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки — правила пары, сроки ожидания, отказ от пробного ордера. Вызывают
/// И настоящий ордер (trade.rs), И проверка без исполнения (check_order).
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила), NotConfigured (настройки; venue.order_test_first = да), BadResponse (пары нет).
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "venue.order_test_first = да, а пробного ордера у Poloniex нет (Spot → API → Orders) — выключите",
        )));
    }
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let buy = req.side == Side::Buy;
    let client_id = client_id();
    let body = json!({
        "symbol": rules.symbol,
        "side": if buy { "BUY" } else { "SELL" },
        "type": "LIMIT",
        "timeInForce": tif,
        "price": price.normalize().to_string(),
        "quantity": qty.normalize().to_string(),
        "clientOrderId": client_id,
        "allowBorrow": false,
    });
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_id,
        received: if buy { rules.base } else { rules.quote },
        body,
    })
}

/// Свой ID (решение 114): новый у каждой попытки — 13 цифр времени (мс) и 4 цифры номера попытки в этой мс; ровно
/// CLIENT_ID_LEN цифр (подмножество A-Za-z0-9_-). Тот же вид — у withdrawOrderId вывода («up to 32 chars»). Ошибок нет.
pub fn client_id() -> String {
    let ms = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(10_000_000_000_000);
    let n = SEQ.fetch_add(1, Ordering::Relaxed) % 10_000;
    format!("{ms:013}{n:04}")
}

/// Округление до шага (шаг 0 — без округления). Ошибок нет.
pub fn round(v: Decimal, step: Decimal, how: RoundingStrategy) -> Decimal {
    if step.is_zero() {
        return v;
    }
    ((v / step).round_dp_with_strategy(0, how) * step).normalize()
}
