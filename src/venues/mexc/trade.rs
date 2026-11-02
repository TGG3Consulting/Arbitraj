//! Файл: venues/mexc/trade.rs
//! Что делает: покупка или продажа на MEXC ордером IMMEDIATE_OR_CANCEL с худшей допустимой ценой (что не исполнилось
//! сразу — снимает биржа) и проверкой факта (количество, средняя цена, комиссия — order_state.rs).
//! Перед ордером — пробный ордер (POST /api/v3/order/test), если включено venue.order_test_first [mexc]: отказ пробного —
//! настоящий не ставится (решение 107: шаги цены и количества MEXC — толкование целых precision).
//! Кто вызывает: mod.rs (trade из контракта).
//! От чего зависит: rest.rs, symbols.rs, num.rs, ctx.rs, order_state.rs, fills.rs.
//! Настройки: venue.order_type [mexc] (должен быть в orderTypes пары), venue.order_test_first [mexc];
//! ожидание итога — order_state.rs, venue.my_trades_limit — fills.rs (проверяются до отправки).
//!
//! Официальная документация:
//! - POST /api/v3/order https://www.mexc.com/api-docs/spot-v3/spot-account-trade/new-order — symbol, side, type,
//!   quantity, price, newClientOrderId; «Rate limit: Shared with the cancel order endpoint, 12 requests/second»;
//!   ответ — orderId без статуса и исполнения (итог — запросом ордера).
//! - Типы ордера https://www.mexc.com/api-docs/spot-v3/public-api-definitions — LIMIT, MARKET, LIMIT_MAKER,
//!   IMMEDIATE_OR_CANCEL, FILL_OR_KILL (параметра timeInForce нет: IOC — это type).
//! - POST /api/v3/order/test — «Test order requests without submitting them to the matching engine», ответ {}.
//!
//! Фильтр PERCENT_PRICE_BY_SIDE (exchangeInfo): лимитная цена не дальше заданного процента от последней сделки —
//! последней цены у нас нет; нарушение — отказ MEXC (OrderRejected), деньги не двигаются.

use reqwest::Method;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_state, rest};

/// Длина newClientOrderId — предел в документации не указан; берём как у BingX: буквы, цифры, «_», до 32.
const MAX_CLIENT_ID: usize = 32;

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (пара не торгуется, сумма вне пределов,
/// отказ пробного ордера, отказ MEXC), OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let buy = req.side == Side::Buy;
    if !rules.tradable(buy) {
        return Err(reject(
            ctx,
            format!(
                "{}: пара не торгуется через API в эту сторону (selfSymbols ключа, isSpotTradingAllowed, tradeSideType)",
                rules.symbol
            ),
        ));
    }
    let qty = round(req.qty, rules.step_size, RoundingStrategy::ToZero);
    let price = round(
        req.worst_price,
        rules.tick_size,
        if buy {
            RoundingStrategy::ToZero
        } else {
            RoundingStrategy::AwayFromZero
        },
    );
    if qty <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(reject(
            ctx,
            format!(
                "{}: после округления количество {qty} или цена {price} не больше нуля",
                rules.symbol
            ),
        ));
    }
    if let Some(min) = rules.min_qty
        && qty < min
    {
        return Err(reject(
            ctx,
            format!(
                "{}: количество {qty} меньше минимального {min}",
                rules.symbol
            ),
        ));
    }
    let notional = qty * price;
    if let Some(max) = rules.max_notional
        && notional > max
    {
        return Err(reject(
            ctx,
            format!(
                "{}: сумма {notional} больше максимальной {max}",
                rules.symbol
            ),
        ));
    }
    if let Some(min) = rules.min_notional
        && notional < min
    {
        return Err(reject(
            ctx,
            format!(
                "{}: сумма {notional} меньше минимальной {min}",
                rules.symbol
            ),
        ));
    }
    let order_type = ctx.text("venue.order_type")?;
    // Решение 118, А-🟡7: тип ордера — из orderTypes пары (exchangeInfo); нет в списке — ордер не ставится.
    if let Some(types) = &rules.order_types
        && !types.contains(&order_type)
    {
        return Err(reject(
            ctx,
            format!(
                "{}: тип ордера {order_type} (venue.order_type) не в orderTypes пары ({})",
                rules.symbol,
                types.join(", ")
            ),
        ));
    }
    // Всё, что нужно после постановки (сроки ожидания, limit сделок), — проверяется до неё: после — поздно.
    order_state::times(ctx)?;
    super::fills::limit(ctx)?;
    // Решение 114: свой ID ордера — свой у каждой попытки. Раньше был «<сделка>_s» — повторная продажа той же сделки
    // («Продать сейчас», «Продать и перегнать», повтор режима застрявшей монеты) уходила с тем же ID и MEXC отвечал
    // «duplicate client order id» (сделка 20261102213149681XPL, 02.11). Метка попытки — миллисекунды (9 цифр) в начале,
    // чтобы обрезка по длине не съела её; номер сделки остаётся для поиска в истории ордеров.
    let attempt = chrono::Utc::now()
        .timestamp_millis()
        .rem_euclid(1_000_000_000);
    let client_id: String = format!(
        "{}{attempt:09}_{}",
        if buy { "b" } else { "s" },
        req.deal_id
    )
    .chars()
    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
    .take(MAX_CLIENT_ID)
    .collect();
    let params = vec![
        ("symbol", rules.symbol.clone()),
        ("side", (if buy { "BUY" } else { "SELL" }).to_string()),
        ("type", order_type),
        ("quantity", qty.normalize().to_string()),
        ("price", price.normalize().to_string()),
        ("newClientOrderId", client_id.clone()),
    ];
    // Пробный ордер: проверка параметров без исполнения. Отказ — настоящий не ставится. Код ошибки сохраняется
    // (RateLimited, Unreachable и др. — решение 118, А-🟡8); непонятный ответ MEXC на пробный — OrderRejected.
    if ctx.flag("venue.order_test_first")? {
        rest::signed(
            ctx,
            Method::POST,
            "/api/v3/order/test",
            params.clone(),
            Op::Other,
        )
        .await
        .map_err(|e| {
            let code = match e.code {
                ErrorCode::BadResponse => ErrorCode::OrderRejected,
                c => c,
            };
            let msg = format!("{}: пробный ордер отклонён — {}", rules.symbol, e.message);
            ctx.tag(GatewayError {
                code,
                message: msg,
                ..e
            })
        })?;
    }
    let placed = rest::signed(ctx, Method::POST, "/api/v3/order", params, Op::Trade).await;
    // Решение 118, А-🟠3: HTTP успешен, а orderId нет — итог неизвестен (не обычный сбой: повтор дал бы второй ордер).
    let placed = placed.and_then(|p| {
        num::text_opt(&p, "orderId").ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без orderId в ответе — итог неизвестен",
                    rules.symbol
                ),
            ))
            .with_raw(p.to_string())
        })
    });
    let order_id = match placed {
        Ok(id) => id,
        // Итог неизвестен — поиск по своему ID; не найден — отмена по нему и снова поиск (А-🟠4).
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &rules.symbol, &client_id).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &rules.symbol, &order_id).await?;
    order_state::result(ctx, &rules.symbol, req, &state, order_id, qty).await
}

/// Округление до шага (шаг 0 — без округления).
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
