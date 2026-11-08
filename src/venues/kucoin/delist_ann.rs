//! Файл: venues/kucoin/delist_ann.rs
//! Что делает: анонсы делистинга KuCoin (решение 226) — страницы GET /api/v3/announcements с annType delistings на
//! английском, не старше delist.ann_max_age_h (startTime); отбор только спотовых анонсов вида «KuCoin Will Delist A
//! and B on 2026-10-14» (у фьючерсов — «KuCoin Futures Will Delist … Perpetual Contracts» и тип
//! futures-announcements — не подходят; «Multiple Tokens» — монет в заголовке нет, не разбирается), список монет
//! из заголовка → записи без времени через gateway/delist_ann.rs (только тикеры из справочника KuCoin).
//! Кто вызывает: delist.rs, tests/delist_sources.rs (parse).
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, reply.rs (Pool), gateway/delist_ann.rs.
//! Настройки: delist.ann_max_age_h, delist.ann_max_pages.
//!
//! Официальная документация: Get Announcements GET /api/v3/announcements
//! https://www.kucoin.com/docs-new/rest/spot-trading/market-data/get-announcements — пул Public, вес 20; «the default
//! page search is for announcements within a month»; параметры currentPage, pageSize, annType (Allowed values: …
//! delistings …), lang (en_US), startTime («Announcement online start time (milliseconds)»), endTime; ответ data =
//! {totalNum, totalPage, currentPage, pageSize, items: [{annId, annTitle, annType: [..], annDesc, cTime, language,
//! annUrl}]}. Живой ответ 08.11.2026 (tests/data/delist_2026-11-08/kucoin_announcements_delistings.json): 4 анонса за
//! месяц — «KuCoin Will Delist BLAST on 2026-10-16», «KuCoin Will Delist VENOM and NFP on 2026-10-14», «KuCoin Will
//! Delist Multiple Tokens on 2026-11-05», фьючерсный PTBUSDT/MITOUSDT.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::DelistItem;
use crate::gateway::delist_ann::{self, Ann};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;
use super::reply::Pool;

/// Путь, тип анонсов, исключаемый тип и язык — значения документации KuCoin.
const PATH: &str = "/api/v3/announcements";
const ANN_TYPE: &str = "delistings";
const FUTURES_TYPE: &str = "futures-announcements";
const LANG: &str = "en_US";
/// Источник для человека.
const SOURCE: &str = "KuCoin GET /api/v3/announcements";
/// Начало спотового заголовка и разделитель даты (формат живых анонсов KuCoin).
const HEAD: &str = "KuCoin Will Delist ";
const DATE_SEP: &str = " on ";

/// Что делает: читает страницы анонсов и строит записи. Принимает: окружение, справочник «тикер → символ».
/// Возвращает: Ok(записи) или Err(причина для warnings).
pub async fn items(ctx: &Ctx, known: &HashMap<String, String>) -> Result<Vec<DelistItem>, String> {
    let d = delist_ann::depth(
        ctx.int("delist.ann_max_age_h"),
        ctx.int("delist.ann_max_pages"),
        Utc::now(),
    )?;
    let mut anns = vec![];
    for page in 1..=d.max_pages {
        let q = vec![
            ("currentPage", page.to_string()),
            ("annType", ANN_TYPE.to_string()),
            ("lang", LANG.to_string()),
            ("startTime", d.since.timestamp_millis().to_string()),
        ];
        let data = super::rest::public(ctx, PATH, q, Pool::Public)
            .await
            .map_err(|e| format!("анонсы KuCoin: {}", e.message))?;
        let (got, total) = parse(&data).map_err(|e| format!("анонсы KuCoin: {}", e.message))?;
        anns.extend(got);
        if page >= total {
            break;
        }
    }
    Ok(delist_ann::items(&anns, known, d.since, SOURCE))
}

/// Что делает: разбор data одной страницы. Принимает: data. Возвращает: (спотовые анонсы делистинга, totalPage).
/// Ошибка BadResponse — нет items, у анонса нет annTitle или cTime.
pub fn parse(data: &Value) -> GatewayResult<(Vec<Ann>, u64)> {
    let list = data
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| super::errors::bad("announcements: нет items", &data.to_string()))?;
    let total = data.get("totalPage").and_then(Value::as_u64).unwrap_or(1);
    let mut out = vec![];
    for a in list {
        let types: Vec<&str> = a
            .get("annType")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !types.contains(&ANN_TYPE) || types.contains(&FUTURES_TYPE) {
            continue;
        }
        let title = num::text(a, "annTitle")?;
        let ms = a
            .get("cTime")
            .and_then(Value::as_i64)
            .ok_or_else(|| super::errors::bad("нет cTime", &a.to_string()))?;
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("cTime вне диапазона", &a.to_string()))?;
        let Some(rest) = title.strip_prefix(HEAD) else {
            continue;
        };
        let list = rest.split(DATE_SEP).next().unwrap_or(rest);
        out.push(Ann {
            list: list.to_string(),
            title: title.clone(),
            at,
        });
    }
    Ok((out, total))
}
