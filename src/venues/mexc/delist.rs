//! Файл: venues/mexc/delist.rs
//! Что делает: делистинги MEXC (решение 226, контракт delistings) — пары с котируемой монетой venue.quote_asset, у
//! которых справочник пар говорит о риске делистинга: пауза (status 2), снята (status 3), только продажа или
//! закрытие (tradeSideType 3 / 4) — запись без времени, ядро такую монету на MEXC не торгует («время не сообщено»,
//! решение 175); метка ST (st = true) — запись с признаком st: блокирует, только если delist.st_block MEXC = да
//! (уточнение Тиграна 08.11.2026: ST по умолчанию торгуем). Анонс MEXC «ST Warning» — та же метка ST, не
//! делистинг: отдельно не разбирается.
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
//! списком монет в заголовках нет («MEXC Announcement on ST Warning (November 2026)» — без тикеров). Повторная
//! проверка 08.11 (уточнение решения 226): 15 страниц по 20 анонсов = 19 суток (20.10–08.11), спотовых делистингов
//! нет ни одного — только «ST Warning» (= метка st), «Delisting of … from MEXC Convert» (не спот), фьючерсы и
//! «Tokens Moved to the Assessment Zone» (смысл зоны в API не описан — вопрос Тиграну, docs/TASKS.md).

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

/// Что делает: причина риска делистинга у пары. Принимает: пару exchangeInfo. Возвращает: (текст признака, это
/// метка ST) или None. Пауза, снятие, только продажа сильнее метки ST — они блокируют всегда.
fn reason(s: &Value) -> Option<(&'static str, bool)> {
    match num::text_opt(s, "status").as_deref() {
        Some("2") => return Some(("status 2 (Pause)", false)),
        Some("3") => return Some(("status 3 (offline)", false)),
        _ => {}
    }
    match num::text_opt(s, "tradeSideType").as_deref() {
        Some("3") => return Some(("tradeSideType 3 (только продажа)", false)),
        Some("4") => return Some(("tradeSideType 4 (Close)", false)),
        _ => {}
    }
    matches!(s.get("st"), Some(Value::Bool(true)))
        .then_some(("st = true (метка ST — зона риска делистинга)", true))
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
        let Some((why, st)) = reason(s) else { continue };
        items.push(DelistItem {
            ticker: num::text(s, "baseAsset")?,
            symbol: num::text(s, "symbol")?,
            trading_end: None,
            withdraw_end: None,
            source: format!("{SOURCE}: {why}"),
            st,
        });
    }
    Ok(Delistings::of(items))
}
