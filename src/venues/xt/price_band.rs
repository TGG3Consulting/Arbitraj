//! Файл: venues/xt/price_band.rs
//! Что делает: решение 392 (t180) — ценовой коридор лимитного ордера XT: правило пары из фильтра PROTECTION_LIMIT
//! (symbols_parse.rs price_band) × последняя цена пары.
//! НЕ ПОДКЛЮЧЕНО (ревью t180, Б3): что p ответа ticker/price — та самая «latest transaction price» фильтра,
//! документация прямо не говорит. Контракт price_band у адаптера не реализован (по умолчанию NotApplicable), в
//! справочник ядра правило не отдаётся (symbols.rs: price_band None). Разбор оставлен для живой записи и теста;
//! подключить — после записи ответа и решения владельца (docs/TASKS.md).
//! Кто вызывает: пока только tests/price_band_390.rs; после подключения — mod.rs (контракт price_band).
//! От чего зависит: symbols.rs (правила пары), rest.rs (публичный GET), num.rs, errors.rs, ctx.rs, gateway/price_band.rs.
//! Настройки: нет.
//!
//! Официально:
//! - коридор — PROTECTION_LIMIT (https://doc.xt.com/docs/spot/Market/GetSymbolInformation): только у orderType LIMIT,
//!   от «latestPrice is the latest transaction price»; «If there is no latest transaction price, there will be no
//!   restrictions». Рыночный ордер — свой фильтр PROTECTION_MARKET (maxDeviation от последней цены к лучшей цене
//!   стакана), здесь не используется (рыночная продажа — решение 268, market_sell.rs).
//! - последняя цена — Get latest prices ticker GET /v4/public/ticker/price?symbol= (10/s/ip), result [{s, t, p}]
//!   (https://doc.xt.com/docs/spot/Market/GetLatestPricesTicker, `_market/8tickerPrice.md`). Что p — именно цена
//!   последней сделки (latestPrice фильтра), в документации прямо не сказано (название «latest prices»): проверить на
//!   живом ответе — docs/TASKS.md, вопрос t180.
//!
//! PROTECTION_ONLINE (первые durationSeconds после открытия пары: цена ≤ openPrice × maxPriceMultiple) не учитывается:
//! цены открытия в API нет (docs/TASKS.md).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;
use crate::gateway::price_band::PriceBand;

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::{num, rest};

/// Путь последней цены (документация — шапка).
const PATH: &str = "/v4/public/ticker/price";

/// Что делает: коридор пары сейчас. Принимает: окружение, справочник пар, тикер ядра. Возвращает: None — у пары нет
/// PROTECTION_LIMIT или у XT нет последней цены (по документации — ограничения нет); Some — границы.
/// Ошибки: BadResponse — пары нет в справочнике или ответ не разобран; сбои запроса — коды rest.rs.
pub async fn read(ctx: &Ctx, symbols: &Symbols, ticker: &str) -> GatewayResult<Option<PriceBand>> {
    let rules = symbols.get(ctx, ticker)?;
    let Some(rule) = rules.price_band else {
        return Ok(None);
    };
    let v = rest::public(ctx, PATH, vec![("symbol", rules.symbol.clone())]).await?;
    let Some(last) = latest(&v, &rules.symbol)? else {
        return Ok(None);
    };
    let note = format!(
        "XT {}: последняя цена {} ({PATH})",
        rules.symbol,
        last.normalize()
    );
    Ok(Some(rule.at(Some(last), note)))
}

/// Последняя цена пары из result Get latest prices ticker (массив {s, t, p}). Принимает: result, символ XT.
/// Возвращает: None — пары в ответе нет или p пусто / null (последней цены нет); Some — цена > 0.
/// Ошибка BadResponse — result не массив, p не число или не больше нуля.
pub fn latest(v: &Value, symbol: &str) -> GatewayResult<Option<Decimal>> {
    let list = v
        .as_array()
        .ok_or_else(|| super::errors::bad("ticker/price: result не массив", &v.to_string()))?;
    let Some(row) = list
        .iter()
        .find(|r| r.get("s").and_then(Value::as_str) == Some(symbol))
    else {
        return Ok(None);
    };
    if row.get("p").and_then(Value::as_str) == Some("") {
        return Ok(None);
    }
    match num::dec_opt(row, "p")? {
        Some(p) if p > Decimal::ZERO => Ok(Some(p)),
        Some(_) => Err(super::errors::bad(
            "ticker/price: цена не больше нуля",
            &row.to_string(),
        )),
        None => Ok(None),
    }
}
