//! Файл: venues/okx/order_rules.rs
//! Что делает: правила ордера OKX — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется, тип ордера снимает неисполненное (ioc / fok), количество и цена
//! после округления до шагов, минимум и максимум количества, сроки ожидания итога. Пробного ордера для спота у OKX нет —
//! venue.order_test_first = да — ошибка настройки.
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), тесты.
//! От чего зависит: symbols.rs (правила пары), order_state.rs (сроки), ctx.rs.
//! Настройки: venue.order_type [okx] (ioc или fok), venue.order_test_first [okx] (у OKX не поддерживается); через
//! order_state.rs — venue.order_poll_ms, venue.order_final_wait_ms (без них ордер не ставится: после постановки — поздно).
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Order Book Trading → Trade → POST / Place order», 60 запросов / 2 с,
//! User ID + инструмент): instId, tdMode (cross, isolated, cash, spot_isolated; «cash» — без маржи), clOrdId («up to 32
//! case-sensitive alphanumerics», «a user-defined unique order identifier at the User ID level»), side buy/sell, ordType
//! (market, limit, post_only, fok, ioc, …; ioc — «cancel the remaining unfilled quantity», fok — всё или ничего), sz
//! («Quantity to buy or sell»; у спота-лимита — в базовой монете, tgtCcy — только для market), px («Order price», для
//! limit, post_only, fok, ioc …). Шаги — «Get instruments»: tickSz, lotSz, minSz, maxLmtSz. Минимальной
//! суммы в справочнике нет — отказ биржи 51020; пределы цены — 51137 / 51138 (отказ биржи). «Order precheck»
//! POST /api/v5/trade/order-precheck — только для «Multi-currency margin mode and Portfolio margin mode» (не спот-режим).

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Предел длины clOrdId — официально «up to 32 characters».
const MAX_CL_ORD_ID: usize = 32;
/// Типы ордера, при которых неисполненное снимает биржа (правило Тиграна): ioc и fok из перечня ordType.
const TYPES_ALLOWED: [&str; 2] = ["ioc", "fok"];
/// Режим торговли без маржи — «cash» (tdMode).
const TD_MODE: &str = "cash";

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub cl_ord_id: String,
    /// Тело запроса.
    pub body: Value,
}

/// Что делает: правила пары без сети — торгуется ли, тип ордера, округление, пределы.
/// Принимает: правила пары, сторону, количество, худшую цену, тип ордера (venue.order_type).
/// Возвращает: (количество, цена) после округления. Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    rules: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
    ord_type: &str,
) -> Result<(Decimal, Decimal), String> {
    let s = &rules.symbol;
    if !rules.tradable() {
        return Err(format!(
            "{s}: пара не торгуется (state/ruleType {})",
            rules.state
        ));
    }
    if !TYPES_ALLOWED.contains(&ord_type) {
        return Err(format!(
            "{s}: venue.order_type = {ord_type} — нужен ioc или fok (неисполненное снимает биржа)"
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
        return Err(format!("{s}: количество {qty} меньше minSz {min}"));
    }
    if let Some(max) = rules.max_qty
        && qty > max
    {
        return Err(format!("{s}: количество {qty} больше maxLmtSz {max}"));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки — правила пары, сроки ожидания. Вызывают И настоящий ордер
/// (trade.rs), И проверка без исполнения (check_order).
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила), NotConfigured (настройки; включён пробный ордер), BadResponse (пары нет).
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let ord_type = ctx.text("venue.order_type")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &ord_type)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "venue.order_test_first = да, а пробного ордера для спота у OKX нет (order-precheck — только \
             Multi-currency и Portfolio margin) — снимите настройку",
        )));
    }
    let buy = req.side == Side::Buy;
    let cl_ord_id = cl_ord_id(buy, &req.deal_id);
    let body = json!({
        "instId": rules.symbol,
        "tdMode": TD_MODE,
        "clOrdId": cl_ord_id,
        "side": if buy { "buy" } else { "sell" },
        "ordType": ord_type,
        "px": price.normalize().to_string(),
        "sz": qty.normalize().to_string(),
    });
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        cl_ord_id,
        body,
    })
}

/// Свой ID ордера (решение 114): новый у каждой попытки — метка (мс, 9 цифр) в начале, номер сделки — для поиска.
/// Возвращает: только буквы и цифры, не длиннее 32 (без «-» и «_» — их нет в разрешённом алфавите). Ошибок нет.
pub fn cl_ord_id(buy: bool, deal_id: &str) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    format!("{}{attempt:09}{deal_id}", if buy { "b" } else { "s" })
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_CL_ORD_ID)
        .collect()
}

/// Округление до шага (шаг 0 — без округления). Ошибок нет.
pub fn round(v: Decimal, step: Decimal, how: RoundingStrategy) -> Decimal {
    if step.is_zero() {
        return v;
    }
    ((v / step).round_dp_with_strategy(0, how) * step).normalize()
}
