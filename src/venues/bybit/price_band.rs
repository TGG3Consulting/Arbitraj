//! Файл: venues/bybit/price_band.rs
//! Что делает: решение 392 (t180) — ценовой коридор лимитного ордера спота Bybit сейчас (контракт price_band): границы
//! считает сама Bybit, адаптер их читает — Get Order Price Limit GET /v5/market/price-limit?category=spot&symbol=.
//! Кто вызывает: mod.rs (контракт price_band; ядро — перед покупкой, core/price_band.rs), symbols.rs (rule — признак
//! коридора в SpotPairState).
//! От чего зависит: symbols.rs (символ пары), rest.rs (публичный GET), num.rs, errors.rs, ctx.rs, gateway/price_band.rs.
//! Настройки: нет.
//!
//! Официально (https://bybit-exchange.github.io/docs/v5/market/order-price-limit): category «spot, linear, inverse»;
//! ответ result {symbol, buyLmt «Highest Bid Price», sellLmt «Lowest Ask Price», ts}; «For spot trading order price
//! limit, refer to announcement» (формула от priceLimitRatioX / priceLimitRatioY — только в объявлении, не в API-доке:
//! адаптер её не считает, берёт готовые границы). Записи ответа спота в tests/data нет — docs/TASKS.md «нужна запись».

use serde_json::Value;

use crate::gateway::errors::GatewayResult;
use crate::gateway::price_band::{PriceBand, PriceBandRule};

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::{num, rest};

/// Путь границ цены (документация — шапка).
const PATH: &str = "/v5/market/price-limit";

/// Правило коридора для справочника ядра: границы считает Bybit (множителей и постоянных границ нет) — признак «перед
/// покупкой спросить price_band». Ошибок нет.
pub fn rule() -> PriceBandRule {
    PriceBandRule {
        reference: format!("границы считает Bybit: {PATH} (buyLmt, sellLmt)"),
        ..PriceBandRule::default()
    }
}

/// Что делает: границы цены пары сейчас. Принимает: окружение, справочник пар, тикер. Возвращает: Some — границы
/// (пустое поле — без границы с этой стороны). Ошибки: BadResponse — пары нет или ответ не разобран; сбои — rest.rs.
pub async fn read(ctx: &Ctx, symbols: &Symbols, ticker: &str) -> GatewayResult<Option<PriceBand>> {
    let r = symbols.get(ctx, ticker)?;
    let q = vec![
        ("category", "spot".to_string()),
        ("symbol", r.symbol.clone()),
    ];
    let v = rest::public(ctx, PATH, q).await?;
    Ok(Some(parse(&v, &r.symbol)?))
}

/// Разбор result Get Order Price Limit: buyLmt — верхняя граница покупки, sellLmt — нижняя граница продажи.
/// Принимает: result, символ (для текста). Ошибка BadResponse — поле не число.
pub fn parse(v: &Value, symbol: &str) -> GatewayResult<PriceBand> {
    let get = |f: &str| match num::text_opt(v, f).as_deref() {
        None | Some("") => Ok(None),
        Some(_) => num::dec_opt(v, f),
    };
    Ok(PriceBand {
        buy_max: get("buyLmt")?,
        sell_min: get("sellLmt")?,
        note: format!("Bybit {symbol}: {PATH}"),
        ..PriceBand::default()
    })
}
