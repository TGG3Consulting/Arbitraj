//! Файл: venues/phemex/order_rules.rs
//! Что делает: правила ордера Phemex — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется, срок действия снимает неисполненное (ImmediateOrCancel /
//! FillOrKill), количество и цена после округления до шагов и перевода в масштабированные целые (ровно, без потерь),
//! пределы количества и суммы, сроки ожидания итога — всё до отправки. Пробного ордера у Phemex нет:
//! venue.order_test_first = да — ошибка настройки.
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), тесты.
//! От чего зависит: symbols.rs (правила пары), order_state.rs (сроки), num.rs, ctx.rs, getrandom.
//! Настройки: venue.order_time_in_force [phemex] (ImmediateOrCancel или FillOrKill), venue.order_test_first [phemex]
//! (у Phemex — только «нет»); через order_state.rs — venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официально (Spot REST API, https://phemex-docs.github.io/#place-order-http-put-prefered):
//! - PUT /spot/orders/create?symbol=&clOrdID=&side=&qtyType=&baseQtyEv=&priceEp=&ordType=&timeInForce= — side Buy /
//!   Sell, qtyType ByBase (baseQtyEv), ordType Limit, timeInForce «GoodTillCancel, PostOnly, ImmediateOrCancel,
//!   FillOrKill».
//! - Масштабы (#price-ratio-value-scales): цена — кратна quoteTickSizeEv, количество — кратно baseTickSizeEv.
//! - clOrdID: длина и алфавит в документации не описаны; во всех примерах — UUID («309bcd5c-9f6e-4a68-b775-…»): свой ID
//!   — случайный UUID (версия 4), новый на каждую попытку (решение 114), проверить на первом живом ордере.
//! - Пробного ордера (test order) в разделе Spot REST API нет.

use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::num::scale_up;
use super::order_state;
use super::rest::Params;
use super::symbols::{SymbolRules, Symbols};

/// Сроки действия, при которых неисполненное снимает биржа (правило Тиграна) — из перечня TimeInForce.
const TIF_ALLOWED: [&str; 2] = ["ImmediateOrCancel", "FillOrKill"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub cl_ord_id: String,
    /// Монета, в которой приходит результат: покупка — базовая, продажа — котируемая.
    pub received: String,
    /// Масштабы количества (базовая) и суммы (котируемая) — для разбора итога.
    pub base_scale: u32,
    pub quote_scale: u32,
    /// Параметры строки запроса PUT /spot/orders/create.
    pub query: Params,
}

/// Округлённый ордер: количество и цена настоящими числами и в масштабированных целых.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rounded {
    pub qty: Decimal,
    pub price: Decimal,
    pub qty_ev: i64,
    pub price_ep: i64,
}

/// Что делает: правила пары без сети — торгуется ли, срок действия, округление, перевод в масштаб, пределы.
/// Принимает: правила пары, сторону, количество, худшую цену, срок действия (venue.order_time_in_force).
/// Возвращает: Rounded. Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    r: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
    tif: &str,
) -> Result<Rounded, String> {
    let s = &r.symbol;
    if !r.tradable() {
        return Err(format!(
            "{s}: пара не торгуется (status «{}», код {})",
            r.status_text, r.status
        ));
    }
    if !TIF_ALLOWED.contains(&tif) {
        return Err(format!(
            "{s}: venue.order_time_in_force = {tif} — нужен ImmediateOrCancel или FillOrKill (неисполненное снимает биржа)"
        ));
    }
    let qty = round(qty, r.step_size, RoundingStrategy::ToZero);
    let how = if side == Side::Buy {
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
            "{s}: количество {qty} больше maxBaseOrderSize {max}"
        ));
    }
    let notional = qty * price;
    if let Some(min) = r.min_notional
        && notional < min
    {
        return Err(format!("{s}: сумма {notional} меньше minOrderValue {min}"));
    }
    if let Some(max) = r.max_notional
        && notional > max
    {
        return Err(format!("{s}: сумма {notional} больше maxOrderValue {max}"));
    }
    let qty_ev = scale_up(qty, r.base_scale).map_err(|e| format!("{s}: количество: {e}"))?;
    let price_ep = scale_up(price, r.price_scale).map_err(|e| format!("{s}: цена: {e}"))?;
    if qty_ev % r.step_ev != 0 || price_ep % r.tick_ep != 0 {
        return Err(format!(
            "{s}: baseQtyEv {qty_ev} не кратно {} или priceEp {price_ep} не кратно {}",
            r.step_ev, r.tick_ep
        ));
    }
    Ok(Rounded {
        qty,
        price,
        qty_ev,
        price_ep,
    })
}

/// Что делает: всё, что проходит ордер до постановки — правила пары, сроки ожидания, отказ от пробного ордера. Вызывают
/// И настоящий ордер (trade.rs), И проверка без исполнения (check_order).
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила), NotConfigured (настройки; venue.order_test_first = да), BadResponse (пары нет),
/// Internal (нет источника случайных чисел для своего ID).
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let r = symbols.get(ctx, &req.asset.ticker)?;
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "venue.order_test_first = да, а пробного ордера у Phemex нет (раздел Spot REST API) — выключите",
        )));
    }
    let tif = ctx.text("venue.order_time_in_force")?;
    let x = check_rules(&r, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let buy = req.side == Side::Buy;
    let cl_ord_id = uuid4().map_err(|e| ctx.tag(e))?;
    let query: Params = vec![
        ("symbol", r.symbol.clone()),
        ("clOrdID", cl_ord_id.clone()),
        ("side", if buy { "Buy" } else { "Sell" }.to_string()),
        ("qtyType", "ByBase".to_string()),
        ("baseQtyEv", x.qty_ev.to_string()),
        ("priceEp", x.price_ep.to_string()),
        ("ordType", "Limit".to_string()),
        ("timeInForce", tif),
    ];
    Ok(Prepared {
        symbol: r.symbol,
        qty: x.qty,
        price: x.price,
        cl_ord_id,
        received: if buy { r.base } else { r.quote },
        base_scale: r.base_scale,
        quote_scale: r.quote_scale,
        query,
    })
}

/// Свой ID (решение 114): случайный UUID версии 4 (вид, как в примерах документации). Новый у каждой попытки.
/// Ошибка Internal — нет системного источника случайных чисел.
pub fn uuid4() -> GatewayResult<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| {
        GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("генератор случайных чисел: {e}"),
        )
    })?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

/// Округление до шага (шаг 0 — без округления). Ошибок нет.
pub fn round(v: Decimal, step: Decimal, how: RoundingStrategy) -> Decimal {
    if step.is_zero() {
        return v;
    }
    ((v / step).round_dp_with_strategy(0, how) * step).normalize()
}
