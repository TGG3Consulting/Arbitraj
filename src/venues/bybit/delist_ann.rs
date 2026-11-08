//! Файл: venues/bybit/delist_ann.rs
//! Что делает: анонсы делистинга Bybit (решение 226) — страницы GET /v5/announcements/index с type delistings и tag
//! Spot на английском; отбор только анонсов с тегом Spot и заголовком «Delisting of A, B and C» (необязательный
//! хвост « on Aug 18, 2026» отрезается); «Delisting 11 Spot trading pairs…», «Spot Token Delisting Notice» — монет в
//! заголовке нет, не разбираются; «Delisting of EPICUSDT Perpetual Contract», «Discontinuation of … as Collateral» —
//! без тега Spot. Список монет → записи без времени через gateway/delist_ann.rs (только тикеры из справочника Bybit).
//! Кто вызывает: delist.rs, tests/delist_sources.rs (parse).
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, gateway/delist_ann.rs.
//! Настройки: delist.ann_max_age_h, delist.ann_max_pages.
//!
//! Официальная документация: Get Announcement GET /v5/announcements/index
//! https://bybit-exchange.github.io/docs/v5/announcement — параметры locale (обязателен), type, tag, page («Default:
//! 1»), limit («Default: 20»); ответ result = {total, list: [{title, description, type: {title, key}, tags, url,
//! dateTimestamp, startDataTimestamp, endDataTimestamp, publishTime}]}. Живой ответ 08.11.2026 с type=delistings и
//! tag=Spot — 59 анонсов по убыванию dateTimestamp: «Delisting of VANRY on Aug 18, 2026», «Delisting of SOLO»,
//! «Delisting of ELX, ODOS and DMAIL», «Delisting of BTC/BRZ and USDT/BRZ», «Delisting of leveraged token pairs».
//! Возраст анонса — по publishTime.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::DelistItem;
use crate::gateway::delist_ann::{self, Ann};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Путь, язык, тип и тег — значения документации и живого ответа Bybit.
const PATH: &str = "/v5/announcements/index";
const LOCALE: &str = "en-US";
const ANN_TYPE: &str = "delistings";
const TAG: &str = "Spot";
/// Источник для человека.
const SOURCE: &str = "Bybit GET /v5/announcements/index";
/// Начало заголовка и разделитель даты (формат живых анонсов Bybit).
const HEAD: &str = "Delisting of ";
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
            ("locale", LOCALE.to_string()),
            ("type", ANN_TYPE.to_string()),
            ("tag", TAG.to_string()),
            ("page", page.to_string()),
        ];
        let data = super::rest::public(ctx, PATH, q)
            .await
            .map_err(|e| format!("анонсы Bybit: {}", e.message))?;
        let (got, oldest) = parse(&data).map_err(|e| format!("анонсы Bybit: {}", e.message))?;
        anns.extend(got);
        if oldest.is_none_or(|t| t < d.since) {
            break;
        }
    }
    Ok(delist_ann::items(&anns, known, d.since, SOURCE))
}

/// Что делает: разбор result одной страницы. Принимает: result. Возвращает: (спотовые анонсы делистинга, время
/// самого старого анонса страницы; None — страница пуста). Ошибка BadResponse — нет list, у анонса нет title или
/// publishTime.
pub fn parse(result: &Value) -> GatewayResult<(Vec<Ann>, Option<DateTime<Utc>>)> {
    let list = result
        .get("list")
        .and_then(Value::as_array)
        .ok_or_else(|| super::errors::bad("announcements: нет list", &result.to_string()))?;
    let (mut out, mut oldest) = (vec![], None::<DateTime<Utc>>);
    for a in list {
        let title = num::text(a, "title")?;
        let ms = a
            .get("publishTime")
            .and_then(Value::as_i64)
            .ok_or_else(|| super::errors::bad("нет publishTime", &a.to_string()))?;
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("publishTime вне диапазона", &a.to_string()))?;
        oldest = Some(oldest.map_or(at, |o| o.min(at)));
        let key = a
            .get("type")
            .and_then(|t| t.get("key"))
            .and_then(Value::as_str);
        let spot = a
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|t| t.iter().any(|x| x.as_str() == Some(TAG)));
        if key != Some(ANN_TYPE) || !spot {
            continue;
        }
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
    Ok((out, oldest))
}
