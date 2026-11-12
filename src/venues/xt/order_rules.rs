//! Файл: venues/xt/order_rules.rs
//! Что делает: правила ордера XT.com — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара ONLINE и торгуется через API (tradingEnabled, openapiEnabled), тип LIMIT
//! есть в orderTypes, timeInForce IOC или FOK (venue.order_time_in_force) и есть в timeInForces пары, количество вниз
//! до шага QUANTITY, цена до шага PRICE в безопасную сторону (шаг — от минимума, как в документации), пределы цены и
//! количества, минимум суммы QUOTE_QTY; настройки ожидания итога. Пробного ордера у XT нет — venue.order_test_first =
//! да для XT — ошибка настройки. Признак «биржа не примет наш ордер на паре» (order_block, решение 254) — та же
//! проверка типа, ею pairs.rs помечает пару в справочнике для ядра (отсев до расчёта).
//! Кто вызывает: trade.rs, mod.rs (check_order), pairs.rs (order_block).
//! От чего зависит: symbols.rs, order_state.rs, ctx.rs.
//! Настройки: venue.order_time_in_force [xt], venue.order_test_first [xt]; через order_state.rs — venue.order_poll_ms,
//! venue.order_final_wait_ms.
//!
//! Официально:
//! - POST /v4/order https://doc.xt.com/docs/spot/Order/SubmitOrder: symbol, side BUY/SELL, type LIMIT,
//!   timeInForce (обязателен: GTC, FOK, IOC, GTX), bizType SPOT (обязателен), price и quantity (type number),
//!   clientOrderId «Pattern: ^[a-zA-Z0-9_]{4,22}» (решение 263, сверено 13.11; было {4,32}); 20/s/apikey. IOC
//!   (https://doc.xt.com/docs/spot/Access Description/PublicModule) — «Cancel the part that cannot be transacted
//!   immediately».
//! - фильтры https://doc.xt.com/docs/spot/Market/GetSymbolInformation: PRICE, QUANTITY (min, max, tickSize; null — без ограничения; шаг от min),
//!   QUOTE_QTY («For orders of the LIMIT type … price*quantity>=min»). PROTECTION_LIMIT / PROTECTION_ONLINE зависят от
//!   последней цены сделки и цены открытия — не проверяются (отказ ORDER_F04xx/F05xx — до движения денег).
//! - Пробного ордера (test order) в документации и SDK pyxt нет.

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Number, Value, json};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_state;
use super::symbols::{SymbolRules, Symbols};

/// Номер попытки в пределах одной миллисекунды — для уникальности своего ID (как у Poloniex).
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Допустимые timeInForce для правила Тиграна «не исполнился — отменяем» (publicModule.md: IOC, FOK снимают остаток сами).
const TIME_IN_FORCE: [&str; 2] = ["IOC", "FOK"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    /// Количество и цена после округления.
    pub qty: Decimal,
    pub price: Decimal,
    /// Свой ID ордера (clientOrderId) — новый на каждую попытку (решение 114).
    pub client_id: String,
    /// Тело POST /v4/order.
    pub body: Value,
}

/// Что делает: примет ли XT на паре наш ордер — тип LIMIT есть в orderTypes, timeInForce (venue.order_time_in_force)
/// есть в timeInForces (решение 254: тот же признак отсеивает пару ДО расчёта — pairs.rs, и страхует ордер —
/// check_rules). Поле отсутствует (null) — не ограничивает. Пустой список [] — как в ответе: значения нет (живой
/// справочник 06.11.2026 — у pepe_usdt orderTypes [] и timeInForces [], у btc_usdt [LIMIT, MARKET] и [GTC, IOC];
/// tests/data/xt_live_2026-11-06). Принимает: правила пары, timeInForce из настройки (None — не задан: сверяется
/// только LIMIT; недопустимое значение настройки — ошибка настройки, её показывает check_rules, а не пара).
/// Возвращает: None — примет; Some — причина. Ошибок нет.
/// Официально: https://doc.xt.com/docs/spot/Market/GetSymbolInformation (orderTypes, timeInForces пары),
/// https://doc.xt.com/docs/spot/Order/SubmitOrder.
pub fn order_block(r: &SymbolRules, tif: Option<&str>) -> Option<String> {
    let s = &r.symbol;
    let lacks =
        |l: &Option<Vec<String>>, v: &str| l.as_ref().is_some_and(|l| !l.iter().any(|x| x == v));
    if lacks(&r.order_types, "LIMIT") {
        return Some(format!("{s}: типа LIMIT нет в orderTypes пары"));
    }
    match tif {
        Some(t) if TIME_IN_FORCE.contains(&t) && lacks(&r.time_in_forces, t) => {
            Some(format!("{s}: {t} нет в timeInForces пары"))
        }
        _ => None,
    }
}

/// Что делает: правила пары без сети. Принимает: правила, сторону, количество, худшую цену, timeInForce.
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
            "{s}: пара не торгуется через API (state {}, tradingEnabled {}, openapiEnabled {})",
            r.state_text, r.trading_enabled, r.openapi_enabled
        ));
    }
    if !TIME_IN_FORCE.contains(&tif) {
        return Err(format!(
            "venue.order_time_in_force = {tif}: у XT для правила «не исполнился — отменяем» только IOC или FOK"
        ));
    }
    if let Some(why) = order_block(r, Some(tif)) {
        return Err(why);
    }
    let qty = r.qty.round(qty, RoundingStrategy::ToZero);
    let how = if buy {
        RoundingStrategy::ToZero
    } else {
        RoundingStrategy::AwayFromZero
    };
    let price = r.price.round(worst, how);
    if qty <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(format!(
            "{s}: после округления количество {qty} или цена {price} не больше нуля"
        ));
    }
    if r.price.outside(price) {
        return Err(format!(
            "{s}: цена {price} вне фильтра PRICE ({:?} … {:?})",
            r.price.min, r.price.max
        ));
    }
    if r.qty.outside(qty) {
        return Err(format!(
            "{s}: количество {qty} вне фильтра QUANTITY ({:?} … {:?})",
            r.qty.min, r.qty.max
        ));
    }
    let notional = qty * price;
    if r.quote_min.is_some_and(|m| notional < m) {
        return Err(format!(
            "{s}: сумма {notional} меньше QUOTE_QTY.min {:?}",
            r.quote_min
        ));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки (правила, настройки итога). Вызывают И ордер, И проверка без
/// исполнения. Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос.
/// Возвращает: Prepared. Ошибки: OrderRejected (правила), NotConfigured, BadResponse (пары нет).
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.order_test_first",
            "venue.order_test_first = да, но пробного ордера у XT нет (документация, SDK pyxt) — поставьте «нет»",
        )));
    }
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    let buy = req.side == Side::Buy;
    let client_id = client_id(buy);
    let body = json!({
        "symbol": rules.symbol,
        "side": if buy { "BUY" } else { "SELL" },
        "type": "LIMIT",
        "timeInForce": tif,
        "bizType": "SPOT",
        "price": number(price),
        "quantity": number(qty),
        "clientOrderId": client_id,
    });
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_id,
        body,
    })
}

/// Decimal → число JSON без потери точности (serde_json с arbitrary_precision). Ошибок нет (Decimal всегда число).
pub fn number(d: Decimal) -> Value {
    Number::from_str(&d.normalize().to_string())
        .map(Value::Number)
        .unwrap_or_else(|_| Value::String(d.normalize().to_string()))
}

/// Свой ID ордера (решения 114, 263): новый у каждой попытки — сторона (b/s), 13 цифр времени (мс) и 4 цифры номера
/// попытки в этой мс: 18 знаков, подходит под «^[a-zA-Z0-9_]{4,22}» (Submit order). Номер сделки в ID больше не входит
/// (не помещается в 22 знака); итог ищется по этому ID целиком — он хранится в «чем найти» (trade.rs PendingRef,
/// order_state::recover, resolve.rs). Принимает: покупка ли. Возвращает: ID. Ошибок нет.
pub fn client_id(buy: bool) -> String {
    let ms = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(10_000_000_000_000);
    let n = SEQ.fetch_add(1, Ordering::Relaxed) % 10_000;
    format!("{}{ms:013}{n:04}", if buy { "b" } else { "s" })
}
