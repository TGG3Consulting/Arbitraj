//! Файл: venues/htx/order_rules.rs
//! Что делает: правила ордера HTX — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется (state online), тип ордера снимает неисполненное (IOC / FOK),
//! количество и цена после округления до шагов, минимум и максимум количества, минимальная сумма, спот-счёт открыт,
//! сроки ожидания итога — всё до отправки. Пробного ордера у HTX нет: venue.order_test_first = да — ошибка настройки.
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), transfers.rs
//! (client_oid), тесты.
//! От чего зависит: symbols.rs (правила пары), accounts.rs (id спот-счёта), order_state.rs (сроки), ctx.rs.
//! Настройки: venue.order_time_in_force [htx] (ioc или fok), venue.order_test_first [htx] (у HTX — только «нет»);
//! через order_state.rs — venue.order_poll_ms, venue.order_final_wait_ms (без них ордер не ставится: после — поздно).
//!
//! Официально — Place a New Order POST /v1/order/orders/place («100 times/2s», право Trade)
//! https://huobiapi.github.io/docs/spot/v1/en/#place-a-new-order: account-id*, symbol*, type* (buy-ioc / sell-ioc —
//! лимит с немедленной отменой остатка, buy-limit-fok / sell-limit-fok — всё или ничего; срок действия — часть type),
//! amount* (у лимитных — количество базовой монеты), price, source («spot-api» по умолчанию), client-order-id («maximum
//! 64 characters», буквы с учётом регистра, цифры, «_», «-»; уникален 8 часов, в таблице ошибок — 24 часа
//! invalid-client-order-id: противоречие документации, свой ID всё равно новый на каждую попытку — решение 114).
//! Пробного ордера (test order) в разделе Trading нет.

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::accounts::SpotId;
use super::ctx::Ctx;
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Предел длины client-order-id ордера — «maximum 64 characters».
pub const MAX_ORDER_CID: usize = 64;
/// Источник ордера — значение source документации («spot-api», по умолчанию).
pub(super) const SOURCE: &str = "spot-api";

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    pub qty: Decimal,
    pub price: Decimal,
    pub client_oid: String,
    /// Монета, в которой приходит результат (и комиссия без скидки): покупка — базовая, продажа — котируемая.
    pub received: String,
    /// Тело запроса Place a New Order.
    pub body: Value,
}

/// Что делает: тип ордера HTX по стороне и сроку действия (venue.order_time_in_force).
/// Возвращает: buy-ioc / sell-ioc / buy-limit-fok / sell-limit-fok. Ошибка — текст причины (другой срок).
pub fn order_type(side: Side, tif: &str) -> Result<String, String> {
    let s = if side == Side::Buy { "buy" } else { "sell" };
    match tif {
        "ioc" => Ok(format!("{s}-ioc")),
        "fok" => Ok(format!("{s}-limit-fok")),
        other => Err(format!(
            "venue.order_time_in_force = {other} — нужен ioc или fok (неисполненное снимает биржа)"
        )),
    }
}

/// Что делает: правила пары без сети — торгуется ли, округление, пределы количества и суммы.
/// Принимает: правила пары, сторону, количество, худшую цену. Возвращает: (количество, цена) после округления.
/// Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    rules: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
) -> Result<(Decimal, Decimal), String> {
    let s = &rules.symbol;
    if !rules.tradable() {
        return Err(format!("{s}: пара не торгуется (state = {})", rules.state));
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
        return Err(format!("{s}: количество {qty} меньше минимума {min}"));
    }
    let side_max = if side == Side::Buy {
        rules.max_buy
    } else {
        rules.max_sell
    };
    if let Some(max) = [rules.max_qty, side_max].into_iter().flatten().min()
        && qty > max
    {
        return Err(format!("{s}: количество {qty} больше максимума {max}"));
    }
    let notional = qty * price;
    if let Some(min) = rules.min_notional
        && notional < min
    {
        return Err(format!("{s}: сумма {notional} меньше minov {min}"));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки. Вызывают И настоящий ордер (trade.rs), И проверка без исполнения.
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), id спот-счёта, запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила), NotConfigured (настройки; venue.order_test_first = да; нет спот-счёта),
/// BadResponse (пары нет), связь (список счетов при первом вызове).
pub async fn ready(
    ctx: &Ctx,
    symbols: &Symbols,
    spot: &SpotId,
    req: &TradeRequest,
) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.order_test_first",
            "venue.order_test_first = да, а пробного ордера у HTX нет (раздел Trading) — выключите",
        )));
    }
    let tif = ctx.text("venue.order_time_in_force")?;
    let kind = order_type(req.side, &tif).map_err(|m| reject(ctx, m))?;
    let (qty, price) =
        check_rules(&rules, req.side, req.qty, req.worst_price).map_err(|m| reject(ctx, m))?;
    order_state::times(ctx)?;
    let account = spot.require(ctx).await?;
    let buy = req.side == Side::Buy;
    let client_oid = client_oid(if buy { 'b' } else { 's' }, &req.deal_id, MAX_ORDER_CID);
    let body = json!({
        "account-id": account.to_string(),
        "symbol": rules.symbol,
        "type": kind,
        "amount": qty.normalize().to_string(),
        "price": price.normalize().to_string(),
        "source": SOURCE,
        "client-order-id": client_oid,
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
/// Принимает: вид, номер сделки, предел длины. Возвращает: буквы, цифры, «_», «-» не длиннее max. Ошибок нет.
pub fn client_oid(kind: char, deal_id: &str, max: usize) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    format!("{kind}{attempt:09}_{deal_id}")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(max)
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
