//! Файл: venues/okx/delist_ann.rs
//! Что делает: анонсы делистинга OKX (решение 226) — страницы GET /api/v5/support/announcements с типом
//! announcements-delistings на английском; отбор только спотовых анонсов вида «OKX to delist A, B and C spot trading
//! pairs» (perpetual futures, X-Perp, миграции, «postpone», «selected USD…» — не подходят), список монет из заголовка
//! → записи без времени через gateway/delist_ann.rs (только тикеры из справочника OKX с venue.quote_asset).
//! Кто вызывает: delist.rs, tests/delist_sources.rs (parse).
//! От чего зависит: rest.rs (public_lang), num.rs, errors.rs, ctx.rs, gateway/delist_ann.rs.
//! Настройки: delist.ann_max_age_h (анонсы старше не читаются), delist.ann_max_pages (не больше страниц).
//!
//! Официальная документация: Get announcements GET /api/v5/support/announcements https://www.okx.com/docs-v5/en/ —
//! «the response is sorted by pTime and businessPTime with the most recent first … Every page has 20 records», «When
//! Accept-Language in HTTP header is set to en-US, the response will be in English», «Rate Limit: 5 requests per 2
//! seconds», параметры annType (из GET /api/v5/support/announcement-types; живой ответ 08.11.2026 —
//! «announcements-delistings») и page; ответ data[0] = {details: [{annType, title, url, pTime, businessPTime}],
//! totalPage}. Живой ответ 08.11: totalPage 7; спотовые заголовки — «OKX to delist DORA, ICX, STORJ, ZEUS and ELF spot
//! trading pairs», «OKX to delist GODS, PRCL and DUCK spot trading pairs». Время делистинга — только в тексте статьи.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::DelistItem;
use crate::gateway::delist_ann::{self, Ann};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Путь, тип анонсов и язык — значения документации OKX.
const PATH: &str = "/api/v5/support/announcements";
const ANN_TYPE: &str = "announcements-delistings";
const LANG: &str = "en-US";
/// Источник для человека.
const SOURCE: &str = "OKX GET /api/v5/support/announcements";
/// Начало и конец спотового заголовка (формат живых анонсов OKX на английском).
const HEAD: &str = "OKX to delist ";
const TAILS: [&str; 2] = [" spot trading pairs", " spot trading pair"];

/// Что делает: читает страницы анонсов (не старше delist.ann_max_age_h, не больше delist.ann_max_pages) и строит
/// записи. Принимает: окружение, справочник «тикер → символ». Возвращает: Ok(записи) или Err(причина для warnings).
pub async fn items(ctx: &Ctx, known: &HashMap<String, String>) -> Result<Vec<DelistItem>, String> {
    let now = Utc::now();
    let d = delist_ann::depth(
        ctx.int("delist.ann_max_age_h"),
        ctx.int("delist.ann_max_pages"),
        now,
    )?;
    let mut anns = vec![];
    for page in 1..=d.max_pages {
        let q = vec![
            ("annType", ANN_TYPE.to_string()),
            ("page", page.to_string()),
        ];
        let data = super::rest::public_lang(ctx, PATH, q, LANG)
            .await
            .map_err(|e| format!("анонсы OKX: {}", e.message))?;
        let (got, total, oldest) =
            parse(&data).map_err(|e| format!("анонсы OKX: {}", e.message))?;
        anns.extend(got);
        if page >= total || oldest.is_none_or(|t| t < d.since) {
            break;
        }
    }
    Ok(delist_ann::items(&anns, known, d.since, SOURCE))
}

/// Что делает: разбор data одной страницы. Принимает: data. Возвращает: (спотовые анонсы делистинга, totalPage,
/// время самого старого анонса страницы). Ошибка BadResponse — нет details, у анонса нет title или pTime.
pub fn parse(data: &Value) -> GatewayResult<(Vec<Ann>, u64, Option<DateTime<Utc>>)> {
    let first = data
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| super::errors::bad("announcements: data пуст", ""))?;
    let list = first
        .get("details")
        .and_then(Value::as_array)
        .ok_or_else(|| super::errors::bad("announcements: нет details", &first.to_string()))?;
    let total = num::text_opt(first, "totalPage")
        .and_then(|t| t.parse().ok())
        .unwrap_or(1);
    let (mut out, mut oldest) = (vec![], None::<DateTime<Utc>>);
    for a in list {
        let title = num::text(a, "title")?;
        let ms: i64 = num::text(a, "pTime")?
            .parse()
            .map_err(|_| super::errors::bad("pTime не число", &a.to_string()))?;
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("pTime вне диапазона", &a.to_string()))?;
        oldest = Some(oldest.map_or(at, |o| o.min(at)));
        if num::text_opt(a, "annType").as_deref() != Some(ANN_TYPE) {
            continue;
        }
        let Some(rest) = title.strip_prefix(HEAD) else {
            continue;
        };
        let Some(list) = TAILS.iter().find_map(|t| rest.strip_suffix(t)) else {
            continue;
        };
        out.push(Ann {
            list: list.to_string(),
            title: title.clone(),
            at,
        });
    }
    Ok((out, total, oldest))
}
