//! Файл: venues/okx/price_band.rs
//! Что делает: решение 390 (t180) — ценовой коридор лимитного ордера спота OKX сейчас (контракт price_band): границы
//! считает сама OKX, адаптер их читает — Get limit price GET /api/v5/public/price-limit?instId=.
//! Кто вызывает: mod.rs (контракт price_band; ядро — перед покупкой, core/price_band.rs), symbols.rs (rule — признак
//! коридора в SpotPairState).
//! От чего зависит: symbols.rs (instId пары), rest.rs (публичный GET), num.rs, ctx.rs, gateway/price_band.rs.
//! Настройки: нет.
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Public Data → REST API → Get limit price», сверено 17.11.2026):
//! «Retrieve the highest buy limit and lowest sell limit of the instrument», 20 запросов / 2 с по IP; ответ data[]:
//! instType, instId, buyLmt «Highest buy limit» («Return "" when enabled is false»), sellLmt «Lowest sell limit»,
//! ts, enabled «Whether price limit is effective». Что эндпоинт отдаёт и SPOT, следует из «Get instruments»: поля
//! initPxLmtPct / floatPxLmtPct / maxPxLmtPct — «Applicable to SPOT / MARGIN / SWAP / FUTURES», «Use GET
//! /api/v5/public/price-limit for the computed price limits» (в самом разделе пример instId — SWAP). Живого ответа
//! для SPOT в tests/data нет — docs/TASKS.md «нужна запись».

use serde_json::Value;

use crate::gateway::errors::GatewayResult;
use crate::gateway::price_band::{PriceBand, PriceBandRule};

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::{num, rest};

/// Путь границ цены (документация — шапка).
const PATH: &str = "/api/v5/public/price-limit";

/// Правило коридора для справочника ядра: границы считает OKX — признак «перед покупкой спросить price_band».
/// Ошибок нет.
pub fn rule() -> PriceBandRule {
    PriceBandRule {
        reference: format!("границы считает OKX: {PATH} (buyLmt, sellLmt, enabled)"),
        ..PriceBandRule::default()
    }
}

/// Что делает: границы цены пары сейчас. Принимает: окружение, справочник пар, тикер. Возвращает: None — enabled =
/// false (коридор не действует); Some — границы. Ошибки: BadResponse — пары нет или ответ не разобран; сбои — rest.rs.
pub async fn read(ctx: &Ctx, symbols: &Symbols, ticker: &str) -> GatewayResult<Option<PriceBand>> {
    let r = symbols.get(ctx, ticker)?;
    let data = rest::public(ctx, PATH, vec![("instId", r.symbol.clone())]).await?;
    parse(rest::first(&data, PATH)?, &r.symbol)
}

/// Разбор строки data Get limit price. Принимает: строку, instId (для текста). Возвращает: None — enabled = false;
/// Some — buyLmt (верх покупки) и sellLmt (низ продажи), пустое поле — без границы. Ошибка BadResponse — поле не число
/// или enabled не логическое.
pub fn parse(v: &Value, inst: &str) -> GatewayResult<Option<PriceBand>> {
    if !num::flag(v, "enabled")? {
        return Ok(None);
    }
    let get = |f: &str| match num::text_opt(v, f).as_deref() {
        None | Some("") => Ok(None),
        Some(_) => num::dec_opt(v, f),
    };
    Ok(Some(PriceBand {
        buy_max: get("buyLmt")?,
        sell_min: get("sellLmt")?,
        note: format!("OKX {inst}: {PATH}"),
        ..PriceBand::default()
    }))
}
