//! Файл: venues/binance/price_band.rs
//! Что делает: решение 390 (t180) — ценовые коридоры лимитного ордера Binance: разбор фильтров PERCENT_PRICE_BY_SIDE
//! и PERCENT_PRICE из exchangeInfo (filters) и границы сейчас (контракт price_band): множители × опорная цена фильтра.
//! Кто вызывает: symbols_parse.rs (filters), symbols.rs (summary — правило в SpotPairState), mod.rs (read — контракт
//! price_band; ядро — перед покупкой, core/price_band.rs).
//! От чего зависит: rest.rs (публичный GET), num.rs, errors.rs, symbols.rs, ctx.rs, gateway/price_band.rs.
//! Настройки: нет.
//!
//! Официально (https://github.com/binance/binance-spot-api-docs/blob/master/filters.md#percent_price_by_side):
//! - PERCENT_PRICE_BY_SIDE: «A BUY order will pass … price <= average of previous trade prices * bidMultiplierUp, price
//!   >= … * bidMultiplierDown»; «A SELL order … price <= … * askMultiplierUp, price >= … * askMultiplierDown».
//!   PERCENT_PRICE: «price <= average of previous trade prices * multiplierUp», «price >= … * multiplierDown».
//! - Опорная цена обоих фильтров: «When a non-null reference price for the symbol exists, it is used in the filter
//!   evaluation. When a non-null reference price for the symbol does not exist, then the volume weighted average price
//!   over the preceding avgPriceMins minutes is used … If avgPriceMins is 0, then the last price is used».
//! - Референсная цена — GET /api/v3/referencePrice?symbol= (вес 2): referencePrice «Can be null»; −2043 «This symbol
//!   doesn't have a reference price» (https://github.com/binance/binance-spot-api-docs/blob/master/rest-api.md#referenceprice).
//! - Средняя цена — GET /api/v3/avgPrice?symbol= (вес 2): {mins «Average price interval (in minutes)», price}
//!   (rest-api.md#avgprice). Средняя берётся, только если mins ответа равен avgPriceMins фильтра: иначе это средняя
//!   за другой срок — границы не известны, ошибка (ядро считает без коридора, как раньше).
//! - Последняя цена — GET /api/v3/ticker/price?symbol= (вес 2): «Latest price for a symbol» (rest-api.md#tickerprice).
//!
//! Правило исполнения PRICE_RANGE (faqs/price_range_execution_rules.md) ордер не отклоняет: часть taker-исполнения вне
//! диапазона истекает (EXPIRED, EXECUTION_RULE_PRICE_RANGE_EXCEEDED) — здесь не учитывается.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;
use crate::gateway::price_band::{PriceBand, PriceBandRule, tighter_max, tighter_min};

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::{num, rest};

/// Один фильтр коридора пары: правило (множители) и срок средней цены avgPriceMins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub rule: PriceBandRule,
    pub avg_mins: i64,
}

/// Что делает: фильтры коридора пары (сначала PERCENT_PRICE_BY_SIDE, затем PERCENT_PRICE). Принимает: поиск фильтра
/// пары по filterType. Возвращает: список (пусто — фильтров нет). Ошибка BadResponse — поле не число или нет
/// avgPriceMins.
pub fn filters<'a>(f: impl Fn(&str) -> Option<&'a Value>) -> GatewayResult<Vec<Filter>> {
    let mut out = Vec::new();
    let names = [
        (
            "PERCENT_PRICE_BY_SIDE",
            [
                "bidMultiplierUp",
                "bidMultiplierDown",
                "askMultiplierUp",
                "askMultiplierDown",
            ],
        ),
        (
            "PERCENT_PRICE",
            [
                "multiplierUp",
                "multiplierDown",
                "multiplierUp",
                "multiplierDown",
            ],
        ),
    ];
    for (kind, [bu, bd, su, sd]) in names {
        let Some(v) = f(kind) else { continue };
        let rule = PriceBandRule {
            buy_up: num::dec_opt(v, bu)?,
            buy_down: num::dec_opt(v, bd)?,
            sell_up: num::dec_opt(v, su)?,
            sell_down: num::dec_opt(v, sd)?,
            reference: format!(
                "{kind}: референсная цена (GET /api/v3/referencePrice), её нет — средняя за avgPriceMins = {} мин \
                 (GET /api/v3/avgPrice; 0 — последняя цена GET /api/v3/ticker/price)",
                num::int(v, "avgPriceMins")?
            ),
            ..PriceBandRule::default()
        };
        out.push(Filter {
            avg_mins: num::int(v, "avgPriceMins")?,
            rule,
        });
    }
    Ok(out)
}

/// Правило для справочника ядра (SpotPairState.price_band): первый фильтр; при двух — в тексте опорной цены оба
/// (множители — первого: у фильтров могут быть разные сроки средней, складывать их нельзя). None — фильтров нет.
/// Ошибок нет.
pub fn summary(list: &[Filter]) -> Option<PriceBandRule> {
    let first = list.first()?;
    let mut rule = first.rule.clone();
    if list.len() > 1 {
        let all: Vec<&str> = list.iter().map(|f| f.rule.reference.as_str()).collect();
        rule.reference = all.join("; ");
    }
    Some(rule)
}

/// Что делает: коридор пары сейчас — по каждому фильтру его опорная цена, итог — строже из границ. Принимает:
/// окружение, справочник пар, тикер. Возвращает: None — у пары нет фильтров; Some — границы.
/// Ошибки: BadResponse — пары нет, ответ не разобран, mins средней не равен avgPriceMins; сбои запроса — rest.rs.
pub async fn read(ctx: &Ctx, symbols: &Symbols, ticker: &str) -> GatewayResult<Option<PriceBand>> {
    let rules = symbols.get(ctx, ticker)?;
    if rules.price_bands.is_empty() {
        return Ok(None);
    }
    let q = || vec![("symbol", rules.symbol.clone())];
    // Референсная цена — общая для обоих фильтров (один запрос).
    let reference = match rest::public(ctx, "/api/v3/referencePrice", q()).await {
        Ok(v) => reference_price(&v)?,
        Err(e) if super::errors::code_of(&e) == Some(NO_REFERENCE) => None,
        Err(e) => return Err(e),
    };
    let mut band = PriceBand::default();
    let mut notes = Vec::new();
    for f in &rules.price_bands {
        let (price, how) = match reference {
            Some(r) => (r, "referencePrice"),
            None if f.avg_mins == 0 => {
                let v = rest::public(ctx, "/api/v3/ticker/price", q()).await?;
                (num::dec(&v, "price")?, "последняя цена")
            }
            None => {
                let v = rest::public(ctx, "/api/v3/avgPrice", q()).await?;
                (avg_price(&v, f.avg_mins)?, "средняя цена")
            }
        };
        let b = f.rule.at(Some(price), String::new());
        band.buy_max = tighter_max(band.buy_max, b.buy_max);
        band.buy_min = tighter_min(band.buy_min, b.buy_min);
        band.sell_max = tighter_max(band.sell_max, b.sell_max);
        band.sell_min = tighter_min(band.sell_min, b.sell_min);
        notes.push(format!("{how} {}", price.normalize()));
    }
    band.note = format!("Binance {}: {}", rules.symbol, notes.join(", "));
    Ok(Some(band))
}

/// Код Binance «у пары никогда не было референсной цены» (rest-api.md#referenceprice).
const NO_REFERENCE: i64 = -2043;

/// Референсная цена из ответа GET /api/v3/referencePrice. Возвращает: None — referencePrice null; Some — цена > 0.
/// Ошибка BadResponse — поле не число или не больше нуля.
pub fn reference_price(v: &Value) -> GatewayResult<Option<Decimal>> {
    match num::dec_opt(v, "referencePrice")? {
        Some(p) if p > Decimal::ZERO => Ok(Some(p)),
        Some(_) => Err(super::errors::bad(
            "referencePrice не больше нуля",
            &v.to_string(),
        )),
        None => Ok(None),
    }
}

/// Средняя цена из ответа GET /api/v3/avgPrice — только если её срок (mins) равен avgPriceMins фильтра. Принимает:
/// ответ, avgPriceMins. Ошибка BadResponse — срок другой (это не та средняя, что у фильтра), нет полей, цена ≤ 0.
pub fn avg_price(v: &Value, avg_mins: i64) -> GatewayResult<Decimal> {
    let mins = num::int(v, "mins")?;
    if mins != avg_mins {
        return Err(super::errors::bad(
            &format!(
                "avgPrice за {mins} мин, а у фильтра avgPriceMins = {avg_mins} — границы коридора неизвестны"
            ),
            &v.to_string(),
        ));
    }
    let p = num::dec(v, "price")?;
    if p <= Decimal::ZERO {
        return Err(super::errors::bad(
            "avgPrice не больше нуля",
            &v.to_string(),
        ));
    }
    Ok(p)
}
