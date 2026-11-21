//! Файл: venues/gate/order_rules.rs
//! Что делает: правила ордера Gate — ОДНА функция для настоящего ордера (trade.rs) и для проверки ордера без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется через API в эту сторону, время жизни ордера снимает остаток
//! (venue.order_time_in_force: ioc или fok), количество и цена после округления до шагов, минимум и максимум
//! количества и суммы, настройки ожидания итога и поиска ордера (проверяются ДО отправки: после — поздно).
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой).
//! От чего зависит: symbols.rs (правила пары), order_state.rs (сроки), ctx.rs.
//! Настройки: venue.order_time_in_force [gate], venue.order_test_first [gate] (у Gate пробного ордера нет — «да»
//! запрещено), venue.order_poll_ms, venue.order_final_wait_ms, venue.order_history_limit [gate].
//!
//! Официальная документация, API v4 → Spot → «Create an order» POST /spot/orders https://www.gate.com/docs/developers/apiv4/en/
//! (SDK: docs/SpotApi.md#create_order, модель docs/Order.md): text, currency_pair, type (limit | market), account
//! (spot), side (buy | sell), amount («type=limit: measured in the base currency»), price («required when type=limit»),
//! time_in_force — gtc, «ioc: ImmediateOrCancelled, immediate execution or cancellation, taker-only», poc, «fok:
//! FillOrKill, fill in full or cancel in full». Стартовое правило Тиграна «не исполнился — отменяем»: только ioc
//! (остаток снимает биржа, finish_as = ioc) или fok; gtc и poc — отказ правилами. text — «1. prefixed with t- 2. no
//! longer than 28 bytes without t- prefix 3. can only include 0-9, A-Z, a-z, underscore(_), hyphen(-) or dot(.)».
//! Пробного ордера (без исполнения) в спот API v4 нет — проверка без исполнения только правилами; venue.order_test_first
//! = да для Gate — ошибка настройки (урок 4, решение 134).

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Приставка своего ID ордера и предел длины без неё — из официальной документации (поле text).
const TEXT_PREFIX: &str = "t-";
const MAX_TEXT: usize = 28;
/// Время жизни ордера, при котором неисполненный остаток снимает биржа (см. шапку).
pub const CANCELS_REST: [&str; 2] = ["ioc", "fok"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// Символ Gate (CAKE_USDT).
    pub symbol: String,
    /// Количество и цена после округления до шагов пары.
    pub qty: Decimal,
    pub price: Decimal,
    /// Свой ID ордера (text «t-…») — для поиска при неизвестном итоге.
    pub text: String,
    /// Тело POST /spot/orders.
    pub body: Value,
}

/// Что делает: правила пары без сети — сторона через API, время жизни, округление, пределы.
/// Принимает: правила пары, сторону, количество, худшую допустимую цену, время жизни (venue.order_time_in_force).
/// Возвращает: (количество, цена) после округления. Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    r: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
    tif: &str,
) -> Result<(Decimal, Decimal), String> {
    let buy = side == Side::Buy;
    let s = &r.symbol;
    if !r.tradable(buy) {
        return Err(format!(
            "{s}: пара не торгуется через API в эту сторону (trade_status, белый список ключа, buy_start/sell_start)"
        ));
    }
    if !CANCELS_REST.contains(&tif) {
        return Err(format!(
            "venue.order_time_in_force = {tif}: у Gate остаток снимает биржа только при {CANCELS_REST:?}"
        ));
    }
    // Количество — вниз до шага, цена — до шага в безопасную сторону (покупка — вниз, продажа — вверх).
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
    if let Some(min) = r.min_qty.filter(|m| qty < *m) {
        return Err(format!("{s}: количество {qty} меньше минимального {min}"));
    }
    if let Some(max) = r.max_qty.filter(|m| qty > *m) {
        return Err(format!("{s}: количество {qty} больше максимального {max}"));
    }
    let notional = qty * price;
    if let Some(min) = r.min_notional.filter(|m| notional < *m) {
        return Err(format!("{s}: сумма {notional} меньше минимальной {min}"));
    }
    if let Some(max) = r.max_notional.filter(|m| notional > *m) {
        return Err(format!("{s}: сумма {notional} больше максимальной {max}"));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки. Вызывают И настоящий ордер (trade.rs), И проверка без исполнения.
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила), NotConfigured (настройки), BadResponse (пары нет).
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        let m = "venue.order_test_first = да, но у Gate пробного ордера в спот API v4 нет — поставьте «нет»";
        return Err(ctx.tag(GatewayError::not_set(None, "venue.order_test_first", m)));
    }
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| reject(ctx, m))?;
    order_state::times(ctx)?;
    order_state::history_limit(ctx)?;
    let buy = req.side == Side::Buy;
    let text = text(buy, &req.deal_id);
    let body = json!({
        "text": text,
        "currency_pair": rules.symbol,
        "type": "limit",
        "account": "spot",
        "side": if buy { "buy" } else { "sell" },
        "amount": qty.normalize().to_string(),
        "price": price.normalize().to_string(),
        "time_in_force": tif,
    });
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        text,
        body,
    })
}

/// Свой ID ордера (решение 114): новый у каждой попытки — «t-», метка попытки (миллисекунды, 9 цифр) в начале, чтобы
/// обрезка не съела её, и номер сделки; только разрешённые знаки, не длиннее 28 после «t-». Ошибок нет.
pub fn text(buy: bool, deal_id: &str) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    let body: String = format!("{}{attempt:09}_{deal_id}", if buy { "b" } else { "s" })
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(MAX_TEXT)
        .collect();
    format!("{TEXT_PREFIX}{body}")
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
