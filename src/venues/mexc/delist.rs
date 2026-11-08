//! Файл: venues/mexc/delist.rs
//! Что делает: делистинги MEXC (решение 226, контракт delistings) — пары с котируемой монетой venue.quote_asset, у
//! которых справочник пар говорит о риске делистинга: метка ST (st = true), пауза (status 2), снята (status 3),
//! только продажа или закрытие (tradeSideType 3 / 4). Время делистинга MEXC в API не сообщает — запись без времени:
//! ядро такую монету на MEXC не торгует вообще («время не сообщено», решение 175).
//! Живой случай 08.11.2026: бот купил ASS на MEXC перед делистингом; в GET /api/v3/exchangeInfo у ASSUSDT было
//! st: true при status "1" и tradeSideType 1 (49 пар из 1800 с st = true). Правила MEXC: метка ST — монета под угрозой
//! делистинга и может быть удалена в течение нескольких дней (статья MEXC, приведена Тиграном).
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, core/types (Delistings).
//! Настройки: venue.quote_asset.
//!
//! Официальная документация: Exchange Information GET /api/v3/exchangeInfo
//! https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information — «status String status:1 -
//! online, 2 - Pause, 3 - offline», «tradeSideType String tradeSide Type:1 - All, 2 - buy order only, 3 - Sell order
//! only, 4 - Close», «st String symbol st status:false,true» (в живом ответе — bool). Другие источники проверены
//! 08.11 и не годятся: GET /api/v3/symbol/offline — только уже снятые пары (8574 записи, все state 3, будущих
//! offlineTime нет); GET /api/v3/announcements — все анонсы подряд (title, url, postTime), спотовых делистингов со
//! списком монет в заголовках нет («MEXC Announcement on ST Warning (November 2026)» — без тикеров).

use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str = "MEXC GET /api/v3/exchangeInfo";

/// Что делает: запрашивает справочник пар и выбирает пары с риском делистинга. Возвращает: Delistings по фактам API.
/// Ошибки: связь, ответ MEXC, не задана venue.quote_asset, неверный формат.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let info = super::rest::public(ctx, "/api/v3/exchangeInfo", vec![]).await?;
    parse(&info, &quote)
}

/// Что делает: причина риска делистинга у пары. Принимает: пару exchangeInfo. Возвращает: текст признака или None.
fn reason(s: &Value) -> Option<&'static str> {
    if matches!(s.get("st"), Some(Value::Bool(true))) {
        return Some("st = true (метка ST — зона риска делистинга)");
    }
    match num::text_opt(s, "status").as_deref() {
        Some("2") => return Some("status 2 (Pause)"),
        Some("3") => return Some("status 3 (offline)"),
        _ => {}
    }
    match num::text_opt(s, "tradeSideType").as_deref() {
        Some("3") => Some("tradeSideType 3 (только продажа)"),
        Some("4") => Some("tradeSideType 4 (Close)"),
        _ => None,
    }
}

/// Что делает: разбор ответа GET /api/v3/exchangeInfo. Принимает: ответ, котируемую монету. Возвращает: пары с
/// котируемой монетой quote и признаком риска делистинга. Ошибка BadResponse — нет массива symbols или полей пары.
pub fn parse(info: &Value, quote: &str) -> GatewayResult<Delistings> {
    let list = info
        .get("symbols")
        .and_then(Value::as_array)
        .ok_or_else(|| super::errors::bad("exchangeInfo: нет массива symbols", ""))?;
    let mut items = vec![];
    for s in list {
        if num::text_opt(s, "quoteAsset").as_deref() != Some(quote) {
            continue;
        }
        let Some(why) = reason(s) else { continue };
        items.push(DelistItem {
            ticker: num::text(s, "baseAsset")?,
            symbol: num::text(s, "symbol")?,
            trading_end: None,
            withdraw_end: None,
            source: format!("{SOURCE}: {why}"),
        });
    }
    Ok(Delistings::of(items))
}
