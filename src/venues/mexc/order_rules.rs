//! Файл: venues/mexc/order_rules.rs
//! Что делает: правила ордера MEXC — ОДНА функция для настоящего ордера (trade.rs) и для проверки ордера без исполнения
//! (mod.rs → check_order, решение 134): пара торгуется через API в эту сторону, тип ордера допустим на паре (решение
//! 133), количество и цена после округления до шагов (решение 107), минимальное количество, минимальная и максимальная
//! сумма, настройки ожидания итога и myTrades; если включено venue.order_test_first — пробный ордер
//! POST /api/v3/order/test с теми же параметрами. Правила в одном месте — проверка до покупки и ордер не расходятся.
//! Признак «биржа не примет наш ордер на паре» (order_block, решение 254) — та же проверка типа, ею symbols.rs
//! помечает пару в справочнике для ядра (отсев до расчёта).
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой), symbols.rs
//! (order_block).
//! От чего зависит: symbols.rs (правила пары), rest.rs (пробный ордер), order_state.rs, fills.rs, ctx.rs.
//! Настройки: venue.order_type [mexc], venue.order_test_first [mexc]; через order_state.rs и fills.rs — сроки ожидания
//! итога и venue.my_trades_limit (без них ордер не ставится: после постановки — поздно).
//!
//! Официальная документация:
//! - POST /api/v3/order https://www.mexc.com/api-docs/spot-v3/spot-account-trade/new-order — symbol, side, type,
//!   quantity, price, newClientOrderId.
//! - POST /api/v3/order/test https://www.mexc.com/api-docs/spot-v3/spot-account-trade/test-new-order — «Test order
//!   requests without submitting them to the matching engine», параметры как у new order, ответ {}.
//! - Типы ордера https://www.mexc.com/api-docs/spot-v3/public-api-definitions.
//! - Пределы пары — GET /api/v3/exchangeInfo (symbols.rs: baseSizePrecision, quoteAmountPrecision, maxQuoteAmount).
//!
//! Фильтр PERCENT_PRICE_BY_SIDE (лимитная цена не дальше процента от последней сделки) правилами не проверяется:
//! последней цены у адаптера нет. Его ловит пробный ордер (если включён) или отказ самой биржи.

use reqwest::Method;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::{SymbolRules, Symbols};
use super::{order_state, rest};

/// Длина newClientOrderId — предел в документации не указан; берём как у BingX: буквы, цифры, «_», до 32.
const MAX_CLIENT_ID: usize = 32;

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// Символ MEXC (CAKEUSDT).
    pub symbol: String,
    /// Количество и цена после округления до шагов пары.
    pub qty: Decimal,
    pub price: Decimal,
    /// Свой ID ордера (newClientOrderId) — для поиска при неизвестном итоге.
    pub client_id: String,
    /// Параметры запроса POST /api/v3/order (те же ушли в пробный ордер).
    pub params: Vec<(&'static str, String)>,
}

/// Что делает: примет ли MEXC на паре наш тип ордера (решение 133: IOC/FOK — по LIMIT, остальные — по orderTypes).
/// Решение 254: тот же признак отсеивает пару ДО расчёта (symbols.rs → pairs.rs) и страхует ордер (check_rules).
/// Принимает: правила пары, тип из настройки venue.order_type (None — не задан: сверяется LIMIT — все наши типы
/// лимитные). Поля orderTypes нет — не ограничивает. Возвращает: None — примет; Some — причина. Ошибок нет.
/// Официально: exchangeInfo (orderTypes) https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information,
/// типы ордера https://www.mexc.com/api-docs/spot-v3/public-api-definitions.
pub fn order_block(rules: &SymbolRules, order_type: Option<&str>) -> Option<String> {
    let types = rules.order_types.as_ref()?;
    let t = order_type.unwrap_or("LIMIT");
    (!rules.order_type_allowed(t)).then(|| {
        format!(
            "{}: тип ордера {t} (venue.order_type) не в orderTypes пары ({})",
            rules.symbol,
            types.join(", ")
        )
    })
}

/// Что делает: правила пары без сети — сторона через API, округление, пределы, тип ордера.
/// Принимает: правила пары, сторону, количество, худшую допустимую цену, тип ордера (venue.order_type).
/// Возвращает: (количество, цена) после округления до шагов. Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    rules: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
    order_type: &str,
) -> Result<(Decimal, Decimal), String> {
    let buy = side == Side::Buy;
    let s = &rules.symbol;
    if !rules.tradable(buy) {
        return Err(format!(
            "{s}: пара не торгуется через API в эту сторону (selfSymbols ключа, isSpotTradingAllowed, tradeSideType)"
        ));
    }
    // Решение 118, А-🟡7 и решение 133: тип ордера сверяется с orderTypes пары (exchangeInfo); IOC/FOK — по LIMIT.
    if let Some(why) = order_block(rules, Some(order_type)) {
        return Err(why);
    }
    // Количество — вниз до шага, цена — до шага в безопасную сторону (покупка — вниз, продажа — вверх).
    let qty = round(qty, rules.step_size, RoundingStrategy::ToZero);
    let how = if buy {
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
        return Err(format!("{s}: количество {qty} меньше минимального {min}"));
    }
    let notional = qty * price;
    if let Some(max) = rules.max_notional
        && notional > max
    {
        return Err(format!("{s}: сумма {notional} больше максимальной {max}"));
    }
    if let Some(min) = rules.min_notional
        && notional < min
    {
        return Err(format!("{s}: сумма {notional} меньше минимальной {min}"));
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки — правила пары (check_rules), настройки ожидания итога и
/// myTrades, пробный ордер (venue.order_test_first). Вызывают И настоящий ордер (trade.rs), И проверка без исполнения.
/// Принимает: окружение, справочник пар (свежесть — у вызывающего), запрос. Возвращает: Prepared.
/// Ошибки: OrderRejected (правила, отказ пробного ордера), NotConfigured (настройки), BadResponse (пары нет),
/// RateLimited / Unreachable и др. — сбой пробного ордера с его кодом (решение 118, А-🟡8).
pub async fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let order_type = ctx.text("venue.order_type")?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price, &order_type)
        .map_err(|m| reject(ctx, m))?;
    // Всё, что нужно после постановки (сроки ожидания, limit сделок), — проверяется до неё: после — поздно.
    order_state::times(ctx)?;
    super::fills::limit(ctx)?;
    let buy = req.side == Side::Buy;
    let client_id = client_id(buy, &req.deal_id);
    let params = vec![
        ("symbol", rules.symbol.clone()),
        ("side", (if buy { "BUY" } else { "SELL" }).to_string()),
        ("type", order_type),
        ("quantity", qty.normalize().to_string()),
        ("price", price.normalize().to_string()),
        ("newClientOrderId", client_id.clone()),
    ];
    if ctx.flag("venue.order_test_first")? {
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

/// Пробный ордер POST /api/v3/order/test: проверка параметров без исполнения. Код ошибки сохраняется (RateLimited,
/// Unreachable и др. — решение 118, А-🟡8); непонятный ответ MEXC на пробный — OrderRejected. Ошибки: как сказано.
async fn test(ctx: &Ctx, symbol: &str, params: Vec<(&'static str, String)>) -> GatewayResult<()> {
    rest::signed(ctx, Method::POST, "/api/v3/order/test", params, Op::Other)
        .await
        .map(|_| ())
        .map_err(|e| {
            let code = match e.code {
                ErrorCode::BadResponse => ErrorCode::OrderRejected,
                c => c,
            };
            let msg = format!("{symbol}: пробный ордер отклонён — {}", e.message);
            ctx.tag(GatewayError {
                code,
                message: msg,
                ..e
            })
        })
}

/// Свой ID ордера (решение 114): свой у каждой попытки — метка попытки (миллисекунды, 9 цифр) в начале, чтобы
/// обрезка по длине не съела её; номер сделки — для поиска в истории ордеров. Принимает: покупка ли, ID сделки.
/// Возвращает: строку из букв, цифр и «_» не длиннее MAX_CLIENT_ID. Ошибок нет.
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

/// Ошибка «ордер не принят» до отправки.
fn reject(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, msg))
}
