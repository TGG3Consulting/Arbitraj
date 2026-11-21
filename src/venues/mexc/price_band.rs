//! Файл: venues/mexc/price_band.rs
//! Что делает: решение 392 (t180) — ценовой коридор лимитного ордера MEXC сейчас (контракт price_band): правило пары
//! из фильтра PERCENT_PRICE_BY_SIDE (symbols_parse.rs price_band) × последняя цена сделки пары.
//! Кто вызывает: mod.rs (контракт price_band; ядро — перед покупкой, core/price_band.rs).
//! От чего зависит: symbols.rs (правила пары), rest.rs (публичный GET), num.rs, ctx.rs, gateway/price_band.rs.
//! Настройки: нет своих (вес запроса — venue.public_ip_weight_10s: rest.rs public_optional → ip_weight.rs slot_optional;
//! места в окне нет — запрос не уходит, ядро считает без коридора).
//!
//! Официально:
//! - коридор — exchangeInfo, PERCENT_PRICE_BY_SIDE: «lastPrice means using the latest trade price»; покупка «orderPrice
//!   <= lastPrice * (1 + bidMultiplierUp)», продажа «orderPrice >= lastPrice * (1 - askMultiplierDown)»
//!   (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information);
//! - последняя цена — Symbol Price Ticker GET /api/v3/ticker/price?symbol=, ответ {symbol, price}, price — «Last price»
//!   (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/symbol-price-ticker; «Weight(IP): 10»).
//!
//! Рыночный ордер: правило покупки в документации — «only for LIMIT, IMMEDIATE_OR_CANCEL, FILL_OR_KILL»; про продажу
//! тип ордера не назван — действует ли на рыночную продажу, документация не говорит (docs/TASKS.md, вопрос t180).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;
use crate::gateway::price_band::PriceBand;

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::{num, rest};

/// Путь последней цены пары (документация — шапка).
const PATH: &str = "/api/v3/ticker/price";

/// Что делает: коридор пары сейчас. Принимает: окружение, справочник пар, тикер монеты на MEXC. Возвращает: None —
/// у пары нет фильтра коридора; Some — границы. Ошибки: BadResponse — пары нет в справочнике или ответ без цены;
/// сбои запроса — коды rest.rs.
pub async fn read(ctx: &Ctx, symbols: &Symbols, ticker: &str) -> GatewayResult<Option<PriceBand>> {
    let rules = symbols.get(ctx, ticker)?;
    let Some(rule) = rules.price_band else {
        return Ok(None);
    };
    // Ревью t180, Н2: вес 10 — в окне веса IP, без пропуска сверх бюджета (места нет — коридор не читается).
    let v = rest::public_optional(ctx, PATH, vec![("symbol", rules.symbol.clone())]).await?;
    let last = last_price(&v)?;
    let note = format!(
        "MEXC {}: lastPrice {} ({PATH})",
        rules.symbol,
        last.normalize()
    );
    Ok(Some(rule.at(Some(last), note)))
}

/// Последняя цена из ответа Symbol Price Ticker с symbol (объект {symbol, price}). Ошибка BadResponse — нет поля price,
/// оно не число или не больше нуля.
pub fn last_price(v: &Value) -> GatewayResult<Decimal> {
    let p = num::dec(v, "price")?;
    if p <= Decimal::ZERO {
        return Err(super::errors::bad(
            "ticker/price: цена не больше нуля",
            &v.to_string(),
        ));
    }
    Ok(p)
}
