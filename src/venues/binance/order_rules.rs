//! Файл: venues/binance/order_rules.rs
//! Что делает: правила ордера Binance — ОДНА функция для настоящего ордера (trade.rs) и для проверки без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется через API (статус TRADING, isSpotTradingAllowed, права
//! аккаунта), тип LIMIT есть в orderTypes, timeInForce IOC или FOK (venue.order_time_in_force), количество вниз до
//! stepSize, цена до tickSize в безопасную сторону, пределы цены, количества и суммы; настройки ожидания итога и
//! myTrades; при venue.order_test_first — пробный ордер POST /api/v3/order/test с теми же параметрами.
//! Признак «биржа не примет наш ордер на паре» (order_block, решение 254) — та же проверка типа, ею symbols.rs
//! помечает пару в справочнике для ядра (отсев до расчёта).
//! Кто вызывает: trade.rs, mod.rs (check_order), symbols.rs (order_block).
//! От чего зависит: symbols.rs, rest.rs, order_state.rs, fills.rs, ctx.rs.
//! Настройки: venue.order_time_in_force [binance], venue.order_test_first [binance]; через order_state.rs и fills.rs —
//! venue.order_poll_ms, venue.order_final_wait_ms, venue.my_trades_limit.
//!
//! Официально (rest-api «Trading endpoints» https://developers.binance.com/legacy-docs/zh-CN/binance-spot-api-docs/rest-api/trading-endpoints):
//! - POST /api/v3/order — symbol, side, type, timeInForce («Required for LIMIT»), quantity, price, newClientOrderId,
//!   newOrderRespType (ACK, RESULT, FULL; «MARKET и LIMIT по умолчанию FULL» — шлём явно FULL).
//! - Time in force (enums.md): GTC, IOC, FOK. IOC — неисполненный остаток снимает биржа (статус EXPIRED).
//! - POST /api/v3/order/test — «tests the request without submitting it to the matching engine», вес 1, ответ {}.
//! - Фильтры (filters.md): PRICE_FILTER, LOT_SIZE, NOTIONAL/MIN_NOTIONAL. PERCENT_PRICE_BY_SIDE и правило PRICE_RANGE
//!   (faqs/price_range_execution_rules) зависят от средней/референсной цены — их ловит пробный ордер или отказ биржи.
//! - Длина и алфавит newClientOrderId в документации не указаны (только −1100 «legal range is '%s'»): берём буквы,
//!   цифры и «_» до 32 знаков — проверить на живом ответе −1100 (отчёт решения 160).

use reqwest::Method;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::{SymbolRules, Symbols};
use super::{order_state, rest};

/// Длина своего ID: в документации предела нет — 32 знака из букв, цифр и «_» (см. шапку).
const MAX_CLIENT_ID: usize = 32;
/// Допустимые timeInForce для правила Тиграна «не исполнился — отменяем» (enums.md: IOC, FOK снимают остаток сами).
const TIME_IN_FORCE: [&str; 2] = ["IOC", "FOK"];

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub symbol: String,
    /// Количество и цена после округления.
    pub qty: Decimal,
    pub price: Decimal,
    /// Свой ID ордера (newClientOrderId) — новый на каждую попытку (решение 114).
    pub client_id: String,
    /// Параметры POST /api/v3/order (те же ушли в пробный ордер).
    pub params: Vec<(&'static str, String)>,
}

/// Что делает: примет ли Binance на паре наш ордер — тип LIMIT есть в orderTypes пары (решение 254: тот же признак
/// отсеивает пару ДО расчёта — symbols.rs → pairs.rs, и страхует ордер — check_rules). timeInForce в exchangeInfo не
/// перечисляется (orderTypes — только типы; enums.md) — его подтверждает пробный ордер (venue.order_test_first).
/// Поля orderTypes нет — не ограничивает. Принимает: правила пары. Возвращает: None — примет; Some — причина.
/// Ошибок нет. Официально: exchangeInfo https://developers.binance.com/docs/binance-spot-api-docs/rest-api/general-endpoints
/// (orderTypes), enums.md (Order types).
pub fn order_block(r: &SymbolRules) -> Option<String> {
    let types = r.order_types.as_ref()?;
    (!types.iter().any(|t| t == "LIMIT")).then(|| {
        format!(
            "{}: типа LIMIT нет в orderTypes пары ({})",
            r.symbol,
            types.join(", ")
        )
    })
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
            "{s}: пара не торгуется через API (status {}, isSpotTradingAllowed или права аккаунта permissionSets)",
            r.status_text
        ));
    }
    if !TIME_IN_FORCE.contains(&tif) {
        return Err(format!(
            "venue.order_time_in_force = {tif}: у Binance для правила «не исполнился — отменяем» только IOC или FOK"
        ));
    }
    if let Some(why) = order_block(r) {
        return Err(why);
    }
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
    let out = |v: Decimal, lo: Option<Decimal>, hi: Option<Decimal>| {
        lo.is_some_and(|l| v < l) || hi.is_some_and(|h| v > h)
    };
    if out(price, r.min_price, r.max_price) {
        return Err(format!(
            "{s}: цена {price} вне PRICE_FILTER ({:?} … {:?})",
            r.min_price, r.max_price
        ));
    }
    if out(qty, r.min_qty, r.max_qty) {
        return Err(format!(
            "{s}: количество {qty} вне LOT_SIZE ({:?} … {:?})",
            r.min_qty, r.max_qty
        ));
    }
    let notional = qty * price;
    if out(notional, r.min_notional, r.max_notional) {
        return Err(format!(
            "{s}: сумма {notional} вне NOTIONAL ({:?} … {:?})",
            r.min_notional, r.max_notional
        ));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки (правила, настройки итога, пробный ордер). Вызывают И ордер,
/// И проверка без исполнения. Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос.
/// Возвращает: Prepared. Ошибки: OrderRejected (правила, отказ пробного), NotConfigured, BadResponse (пары нет),
/// RateLimited / Unreachable — сбой пробного ордера с его кодом.
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let tif = ctx.text("venue.order_time_in_force")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &tif)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    order_state::times(ctx)?;
    super::fills::limit(ctx)?;
    let buy = req.side == Side::Buy;
    let client_id = client_id(buy, &req.deal_id);
    let params = vec![
        ("symbol", rules.symbol.clone()),
        ("side", (if buy { "BUY" } else { "SELL" }).to_string()),
        ("type", "LIMIT".to_string()),
        ("timeInForce", tif),
        ("quantity", qty.normalize().to_string()),
        ("price", price.normalize().to_string()),
        ("newClientOrderId", client_id.clone()),
        ("newOrderRespType", "FULL".to_string()),
    ];
    if ctx.flag("venue.order_test_first").unwrap_or(false) {
        test(ctx, &rules.symbol, params.clone()).await?;
    }
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        client_id,
        params,
    })
}

/// Пробный ордер POST /api/v3/order/test. Код ошибки сохраняется; непонятный ответ — OrderRejected.
async fn test(ctx: &Ctx, symbol: &str, params: Vec<(&'static str, String)>) -> GatewayResult<()> {
    rest::signed(ctx, Method::POST, "/api/v3/order/test", params, Op::Other)
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

/// Свой ID ордера (решение 114): метка попытки (мс, 9 цифр) в начале, номер сделки — для поиска. Принимает: покупка
/// ли, ID сделки. Возвращает: буквы, цифры и «_», не длиннее MAX_CLIENT_ID. Ошибок нет.
pub fn client_id(buy: bool, deal_id: &str) -> String {
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    format!("{}{attempt:09}_{deal_id}", if buy { "b" } else { "s" })
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
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
