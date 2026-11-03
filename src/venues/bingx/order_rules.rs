//! Файл: venues/bingx/order_rules.rs
//! Что делает: правила ордера BingX — ОДНА функция для настоящего ордера (trade.rs) и для проверки ордера без
//! исполнения (mod.rs → check_order, решение 134): пара торгуется через API в эту сторону (status, apiStateBuy /
//! apiStateSell), количество — вниз до stepSize, цена — до tickSize в безопасную сторону (покупка — вниз, продажа —
//! вверх), количество в пределах minQty / maxQty, сумма в пределах minNotional / maxNotional, задан
//! venue.order_time_in_force. Правила в одном месте — проверка до покупки и ордер не расходятся.
//! Пробного ордера (test order) у спота BingX НЕТ: в официальном справочнике спотовой торговли перечислены order,
//! batchOrders, cancel*, cancelReplace, query, openOrders, historyOrders, myTrades — test есть только у бессрочных
//! фьючерсов (POST /openApi/swap/v2/trade/order/test), фьючерсов в проекте нет. Поэтому у BingX — только правила.
//! Кто вызывает: trade.rs (перед постановкой), mod.rs (check_order — предпроверка ядра перед покупкой).
//! От чего зависит: symbols.rs (правила пары), ctx.rs.
//! Настройки: venue.order_time_in_force [bingx].
//!
//! Официальная документация:
//! - POST /openApi/spot/v1/trade/order — symbol, side (BUY/SELL), type (LIMIT), quantity, price, timeInForce
//!   (GTC / IOC / FOK / PostOnly), newClientOrderId (буквы, цифры, «_», 1–40 символов):
//!   https://github.com/BingX-API/api-ai-skills/blob/main/skills/spot-trade/api-reference.md
//!   (список эндпоинтов спота — skills/spot-trade/SKILL.md того же репозитория; order/test у спота нет).
//! - Пределы пары — «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//!   https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols (tickSize, stepSize,
//!   minQty, maxQty, minNotional, maxNotional, status, apiStateBuy, apiStateSell; minQty / maxQty — решение 66).

use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::symbols::{SymbolRules, Symbols};

/// Предел длины newClientOrderId по документации place order: «1 to 40».
const MAX_CLIENT_ID: usize = 40;

/// Ордер, прошедший правила: что уйдёт на биржу.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// Символ BingX (CAKE-USDT).
    pub symbol: String,
    /// Количество и цена после округления до шагов пары.
    pub qty: Decimal,
    pub price: Decimal,
    /// Параметры запроса POST /openApi/spot/v1/trade/order.
    pub params: Vec<(&'static str, String)>,
}

/// Что делает: правила пары без сети — сторона через API, округление, пределы количества и суммы.
/// Принимает: правила пары, сторону, количество, худшую допустимую цену.
/// Возвращает: (количество, цена) после округления до шагов. Ошибки: текст причины (ордер не будет принят).
pub fn check_rules(
    rules: &SymbolRules,
    side: Side,
    qty: Decimal,
    worst: Decimal,
) -> Result<(Decimal, Decimal), String> {
    let buy = side == Side::Buy;
    let s = &rules.symbol;
    if !rules.tradable(buy) {
        return Err(format!(
            "{s}: пара не торгуется через API в эту сторону (status {}, apiState{})",
            rules.status,
            if buy { "Buy" } else { "Sell" }
        ));
    }
    let qty = round(qty, rules.step_size, RoundingStrategy::ToZero);
    let how = if buy {
        RoundingStrategy::ToZero
    } else {
        RoundingStrategy::AwayFromZero
    };
    let price = round(worst, rules.tick_size, how);
    if qty <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(format!(
            "{s}: после округления по шагам количество {qty} или цена {price} не больше нуля"
        ));
    }
    let sum = qty * price;
    // Решение 66: соблюдаются оба предела — количество (minQty / maxQty) и сумма (minNotional / maxNotional).
    // 0 у минимума — ограничения нет; 0 у максимума — предел неизвестен (как в core/costs_quote.rs), ордера нет.
    let checks = [
        (
            "количество",
            qty,
            rules.min_qty,
            rules.max_qty,
            "minQty",
            "maxQty",
        ),
        (
            "сумма",
            sum,
            rules.min_notional,
            rules.max_notional,
            "minNotional",
            "maxNotional",
        ),
    ];
    for (what, v, min, max, min_name, max_name) in checks {
        if let Some(min) = min
            && v < min
        {
            return Err(format!("{s}: {what} {v} меньше {min_name} {min}"));
        }
        match max {
            Some(m) if m.is_zero() => {
                return Err(format!("{s}: {max_name} = 0 — предел неизвестен"));
            }
            Some(m) if v > m => return Err(format!("{s}: {what} {v} больше {max_name} {m}")),
            _ => {}
        }
    }
    Ok((qty, price))
}

/// Что делает: всё, что проходит ордер до постановки — правила пары (check_rules) и настройка timeInForce. Вызывают
/// И настоящий ордер (trade.rs), И проверка без исполнения. Принимает: окружение, справочник пар (свежесть — у
/// вызывающего), запрос. Возвращает: Prepared. Ошибки: OrderRejected (правила), NotConfigured (venue.order_time_in_force),
/// BadResponse (пары нет в справочнике).
pub fn ready(ctx: &Ctx, symbols: &Symbols, req: &TradeRequest) -> GatewayResult<Prepared> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let (qty, price) = check_rules(&rules, req.side, req.qty, req.worst_price)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, m)))?;
    let tif = ctx.text("venue.order_time_in_force")?;
    let buy = req.side == Side::Buy;
    let params = vec![
        ("symbol", rules.symbol.clone()),
        ("side", (if buy { "BUY" } else { "SELL" }).to_string()),
        ("type", "LIMIT".to_string()),
        ("quantity", qty.normalize().to_string()),
        ("price", price.normalize().to_string()),
        ("timeInForce", tif),
        ("newClientOrderId", client_id(buy, &req.deal_id)),
    ];
    Ok(Prepared {
        symbol: rules.symbol,
        qty,
        price,
        params,
    })
}

/// Свой ID ордера (решение 114): свой у каждой попытки — метка попытки (миллисекунды, 9 цифр) в начале, чтобы обрезка
/// по длине не съела её; номер сделки — для поиска в истории ордеров. Принимает: покупка ли, ID сделки.
/// Возвращает: строку из букв, цифр и «_» не длиннее 40. Ошибок нет.
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
