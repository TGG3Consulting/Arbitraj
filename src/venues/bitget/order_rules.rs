//! Файл: venues/bitget/order_rules.rs
//! Что делает: правила ордера Bitget — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется (status online), срок действия снимает неисполненное (ioc/fok),
//! количество и цена после округления до шагов, минимальная сумма minTradeUSDT, сроки ожидания итога — всё до отправки.
//! Пробного ордера у Bitget Classic нет: venue.order_test_first = да — ошибка настройки (как Gate).
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), transfers.rs
//! (client_oid), тесты.
//! От чего зависит: symbols.rs (правила пары), order_state.rs (сроки), ctx.rs.
//! Настройки: venue.order_time_in_force [bitget] (ioc или fok), venue.order_test_first [bitget] (у Bitget — только «нет»);
//! через order_state.rs — venue.order_poll_ms, venue.order_final_wait_ms (без них ордер не ставится: после — поздно).
//!
//! Официально:
//! - Place Order POST /api/v2/spot/trade/place-order https://www.bitget.com/docs/catalog/classic-spot-trade/classic-spot-trade#place-order
//!   — symbol, side buy/sell, orderType limit, force gtc / post_only / fok / ioc, price, size («For limit orders and
//!   market sells, base coin quantity»), clientOid.
//! - Best Practices (https://www.bitget.com/legacy-docs/classic/best-practices): clientOid «must match
//!   ^[0-9A-Za-z_:#\-+\s]{1,32}$», уникальность — среди открытых ордеров; IOC: «IOC order partially filled then canceled
//!   by system». Новый clientOid на каждую попытку (решение 114).
//! - В разделе Spot → Trade пробного ордера (test order) нет. buyLimitPriceRatio / sellLimitPriceRatio правилами не
//!   проверяются: от какой рыночной цены считать, в документации не определено — отказ даст сама биржа.
//! - Точность цены и количества — pricePrecision / quantityPrecision (шаг 10^−n, symbols_parse.rs).

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Предел длины clientOid — Best Practices: «{1,32}».
const MAX_CLIENT_OID: usize = 32;
/// Сроки действия, при которых неисполненное снимает биржа (правило Тиграна): ioc и fok из перечня force.
const TIF_ALLOWED: [&str; 2] = ["ioc", "fok"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub client_oid: String,
    /// Монета, в которой приходит результат (и комиссия без BGB): покупка — базовая, продажа — котируемая.
    pub received: String,
    /// Тело запроса Place Order.
    pub body: Value,
}

/// Что делает: правила пары без сети — торгуется ли, срок действия, округление, минимальная сумма.
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
            "{s}: пара не торгуется (status не online), код {}",
            rules.status
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
    let notional = qty * price;
    if let Some(min) = rules.min_notional
        && notional < min
    {
        return Err(format!("{s}: сумма {notional} меньше minTradeUSDT {min}"));
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
            "venue.order_test_first = да, а пробного ордера у Bitget Classic нет (раздел Spot → Trade) — выключите",
        )));
    }
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| reject(ctx, m))?;
    order_state::times(ctx)?;
    let buy = req.side == Side::Buy;
    let client_oid = client_oid(if buy { 'b' } else { 's' }, &req.deal_id);
    let body = json!({
        "symbol": rules.symbol,
        "side": if buy { "buy" } else { "sell" },
        "orderType": "limit",
        "force": tif,
        "price": price.normalize().to_string(),
        "size": qty.normalize().to_string(),
        "clientOid": client_oid,
    });
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_oid,
        received: if buy { rules.base } else { rules.quote },
        body,
    })
}

/// Свой ID (решение 114): новый у каждой попытки — вид операции, метка (мс, 9 цифр), номер сделки — для поиска.
/// Возвращает: буквы, цифры, «_», «-» (подмножество разрешённых Best Practices), не длиннее 32. Ошибок нет.
pub fn client_oid(kind: char, deal_id: &str) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    format!("{kind}{attempt:09}_{deal_id}")
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
