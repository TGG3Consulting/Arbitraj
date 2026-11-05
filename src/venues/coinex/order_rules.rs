//! Файл: venues/coinex/order_rules.rs
//! Что делает: правила ордера CoinEx — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется (status online и API-торговля), тип ордера снимает
//! неисполненное (ioc/fok), количество и цена после округления до знаков, минимальное количество, сроки ожидания итога и
//! предел страницы сделок — всё до отправки. Пробного ордера у CoinEx нет: venue.order_test_first = да — ошибка
//! настройки (как Gate, Bitget).
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), transfers.rs
//! (client_id), тесты.
//! От чего зависит: symbols.rs (правила пары), order_state.rs (сроки), ctx.rs.
//! Настройки: venue.order_time_in_force [coinex] (ioc или fok), venue.order_test_first [coinex] (у CoinEx — только «нет»);
//! через order_state.rs — venue.order_poll_ms, venue.order_final_wait_ms, venue.my_trades_limit (без них ордер не
//! ставится: после — поздно, урок 7).
//!
//! Официально:
//! - Place Order POST /spot/order https://docs.coinex.com/api/v2/spot/order/http/put-order — market, market_type SPOT,
//!   side buy/sell, type, amount «Order amount», price, client_id («Up to 32 bytes of letters, digits, hyphens, and
//!   underscores»). Новый client_id на каждую попытку (решение 114; уникальность в документации не описана).
//! - order_type (https://docs.coinex.com/api/v2/enum): limit, market, maker_only, ioc «immediate or cancel», fok «fill
//!   or kill» — срок действия задаётся ТИПОМ ордера, а не отдельным параметром (урок 5, 30: IOC в справочнике пары не
//!   перечислен — проверить живым ордером на минимальную сумму).
//! - Шага цены и количества нет, только число знаков (symbols_parse.rs); фильтры отклонения цены (3606, 3615, 3618 —
//!   от последней или индексной цены) заранее не проверяемы — отказ даст сама биржа.
//! - Пробного ордера (test order) среди эндпоинтов Spot → Order нет.

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Предел длины client_id — «Up to 32 bytes».
pub const MAX_CLIENT_ID: usize = 32;
/// Типы ордера, при которых неисполненное снимает биржа (правило Тиграна): ioc и fok из перечня order_type.
const TIF_ALLOWED: [&str; 2] = ["ioc", "fok"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub client_id: String,
    /// Базовая и котируемая монеты пары (монета комиссии — по полю base_fee / quote_fee, order_fee.rs).
    pub base: String,
    pub quote: String,
    /// Тело запроса Place Order.
    pub body: Value,
}

/// Что делает: правила пары без сети — торгуется ли, тип ордера, округление, минимальное количество.
/// Принимает: правила пары, сторону, количество, худшую цену, тип (venue.order_time_in_force).
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
            "{s}: пара не торгуется через API (status код {}, is_api_trading_available = {})",
            rules.status, rules.api
        ));
    }
    if !TIF_ALLOWED.contains(&tif) {
        return Err(format!(
            "{s}: venue.order_time_in_force = {tif} — нужен ioc или fok (неисполненное снимает биржа)"
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
        return Err(format!("{s}: количество {qty} меньше min_amount {min}"));
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
            "venue.order_test_first = да, а пробного ордера у CoinEx нет (раздел Spot → Order) — выключите",
        )));
    }
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| reject(ctx, m))?;
    order_state::times(ctx)?;
    let buy = req.side == Side::Buy;
    let client_id = client_id(if buy { 'b' } else { 's' }, &req.deal_id);
    let body = json!({
        "market": rules.symbol,
        "market_type": "SPOT",
        "side": if buy { "buy" } else { "sell" },
        "type": tif,
        "amount": qty.normalize().to_string(),
        "price": price.normalize().to_string(),
        "client_id": client_id,
    });
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_id,
        base: rules.base,
        quote: rules.quote,
        body,
    })
}

/// Свой ID (решение 114): новый у каждой попытки — вид операции, метка (мс, 9 цифр), номер сделки — для поиска.
/// Возвращает: буквы, цифры, «_», «-» (алфавит client_id документации), не длиннее 32 байт. Ошибок нет.
pub fn client_id(kind: char, deal_id: &str) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    format!("{kind}{attempt:09}_{deal_id}")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(MAX_CLIENT_ID)
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
