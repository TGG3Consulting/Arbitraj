//! Файл: venues/kraken/order_rules.rs
//! Что делает: правила ордера Kraken — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется, срок действия снимает неисполненное (IOC / FOK), количество и
//! цена после округления до шагов, минимум количества и суммы, сроки ожидания итога; пробный ордер validate=true — при
//! venue.order_test_first = да (probe).
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), тесты.
//! От чего зависит: symbols.rs (правила пары), order_state.rs (сроки), rest.rs, ctx.rs.
//! Настройки: venue.order_time_in_force [kraken] (IOC или FOK), venue.order_test_first [kraken]; через order_state.rs —
//! venue.order_poll_ms, venue.order_final_wait_ms (без них ордер не ставится: после постановки — поздно).
//!
//! Официально — POST /0/private/AddOrder (OpenAPI spot-rest.yaml, схема add): ordertype limit, type buy/sell, pair,
//! price, volume (в базовой монете), timeinforce GTC | IOC | GTD | FOK (FOK — с 05.2026, журнал изменений), cl_ord_id
//! («Free format ascii text up to 18 characters», уникален среди открытых), oflags — fciq «prefer fee in quote currency»
//! (комиссия в котируемой: покупка не уменьшает купленное количество), validate — «the order will be validated only, it
//! will not trade in the matching engine» (проверяет ли баланс — не сказано). leverage не шлётся: маржи у бота нет.
//! Правила пары — AssetPairs: tick_size, lot_decimals, ordermin (базовая), costmin (котируемая), status online.

use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::order_state;
use super::rest::{self, Params, p};
use super::symbols::{SymbolRules, Symbols};

/// Предел длины cl_ord_id — официально «up to 18 characters» (свободный текст).
const MAX_CL_ORD_ID: usize = 18;
/// Сроки действия, при которых неисполненное снимает биржа (правило Тиграна).
const TIF_ALLOWED: [&str; 2] = ["IOC", "FOK"];
/// Комиссия в котируемой монете — флаг oflags «fciq».
pub const FEE_IN_QUOTE: &str = "fciq";

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub cl_ord_id: String,
    /// Поля формы AddOrder (без nonce).
    pub form: Params,
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
        return Err(format!("{s}: пара не торгуется (status {})", rules.state));
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
        return Err(format!("{s}: количество {qty} меньше ordermin {min}"));
    }
    if let Some(min) = rules.min_cost
        && qty * price < min
    {
        return Err(format!("{s}: сумма {} меньше costmin {min}", qty * price));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки — правила пары, сроки ожидания. Вызывают И настоящий ордер
/// (trade.rs), И проверка без исполнения (check_order).
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила), NotConfigured (настройки), BadResponse (пары нет).
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let buy = req.side == Side::Buy;
    let cl_ord_id = cl_ord_id(buy, &req.deal_id);
    let form = vec![
        p("ordertype", "limit"),
        p("type", if buy { "buy" } else { "sell" }),
        p("pair", rules.symbol.clone()),
        p("price", price.normalize().to_string()),
        p("volume", qty.normalize().to_string()),
        p("timeinforce", tif),
        p("oflags", FEE_IN_QUOTE),
        p("cl_ord_id", cl_ord_id.clone()),
    ];
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        cl_ord_id,
        form,
    })
}

/// Что делает: пробный ордер (validate=true) по тем же полям, если venue.order_test_first = да; иначе ничего.
/// Принимает: окружение, готовый ордер. Ошибки: отказ Kraken (OrderRejected и др.), связь.
pub async fn probe(ctx: &Ctx, prepared: &Prepared) -> GatewayResult<()> {
    if !ctx.flag("venue.order_test_first").unwrap_or(false) {
        return Ok(());
    }
    let mut form = prepared.form.clone();
    form.push(p("validate", "true"));
    // Op::Other: пробный ордер не торгует — сбой связи не «итог неизвестен»; отказ Kraken — отказ ордера.
    rest::private(ctx, "AddOrder", form, Op::Other)
        .await
        .map(|_| ())
        .map_err(|mut e| {
            if e.code == ErrorCode::BadResponse && e.message.starts_with(super::errors::PREFIX) {
                e.code = ErrorCode::OrderRejected;
            }
            e
        })
}

/// Свой ID ордера (решение 114): новый у каждой попытки — метка (мс, 9 цифр) в начале, номер сделки — для поиска.
/// Возвращает: только ASCII буквы и цифры, не длиннее 18. Ошибок нет.
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
