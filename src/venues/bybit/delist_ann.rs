//! Файл: venues/bybit/delist_ann.rs
//! Что делает: анонсы делистинга Bybit (решение 226, уточнение Тиграна 08.11.2026) — страницы GET
//! /v5/announcements/index с type delistings на английском (БЕЗ тега Spot: живой ответ 08.11 — спотовые делистинги
//! с сентября 2026 идут с тегами Delistings, Institutions, VIP; тег Spot был только у старых). Отбор: в заголовке
//! «delist» («Delisting of A, B and C[ on …]», «Bybit to Delist N Token(s) / Trading Pair(s) on …», «Spot Token
//! Delisting Notice», «Delisting 11 Spot trading pairs on 2026-07-28 08:00:00 UTC»); анонсы с тегами Derivatives,
//! WEB3 (Bybit Alpha) и заголовки про фьючерсы, залог, займы, Earn (gateway/delist_html::spot) — не спот.
//! Монеты — из заголовка, иначе со страницы url (строки «Delisted trading pairs: HFTUSDT»); время делистинга — из
//! заголовка/description, иначе со страницы («Trading of HFTUSDT pairs … will end after Oct 28, 2026, 8:00AM UTC»).
//! Кто вызывает: delist.rs, tests/delist_sources.rs, tests/delist_ann.rs (parse).
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, gateway/{delist_ann,delist_page,delist_html}.rs.
//! Настройки: delist.ann_max_age_h, delist.ann_max_pages, delist.ann_html_per_refresh, http.* (страницы анонсов).
//!
//! Официальная документация: Get Announcement GET /v5/announcements/index
//! https://bybit-exchange.github.io/docs/v5/announcement — параметры locale (обязателен), type, tag, page («Default:
//! 1»), limit («Default: 20»); ответ result = {total, list: [{title, description, type: {title, key}, tags, url,
//! dateTimestamp, startDataTimestamp, endDataTimestamp, publishTime}]}. Живой ответ 08.11.2026 (type=delistings,
//! 4 страницы по 20 = 80 анонсов с 09.06.2026): за 30 дней 11 анонсов (из них спот — «Bybit to Delist 1 Token(s) on
//! Oct 28, 2026», «… on Oct 13, 2026», «Bybit to Delist 2 Token(s) on Oct 8, 2026»); description у них — общая фраза
//! без монет и времени, dateTimestamp — время публикации, а не делистинга. Возраст анонса — по publishTime.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::DelistItem;
use crate::gateway::delist_ann::{self, Ann, Known};
use crate::gateway::delist_page::Fetch;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Путь, язык и тип — значения документации и живого ответа Bybit.
const PATH: &str = "/v5/announcements/index";
const LOCALE: &str = "en-US";
const ANN_TYPE: &str = "delistings";
/// Теги не спотовых анонсов (живой ответ: фьючерсы — Derivatives, Bybit Alpha — WEB3).
const NOT_SPOT: [&str; 2] = ["Derivatives", "WEB3"];
/// Источник для человека.
const SOURCE: &str = "Bybit GET /v5/announcements/index";
/// Признак делистинга в заголовке (без учёта регистра), начало заголовка с перечнем монет и разделитель даты
/// (формат живых анонсов Bybit).
const MARK: &str = "delist";
const HEAD_LIST: &str = "Delisting of ";
const DATE_SEP: &str = " on ";

/// Что делает: читает страницы анонсов (и, если нужно, страницы самих анонсов) и строит записи. Принимает:
/// окружение, справочник «тикер → символ», котируемую монету. Возвращает: Ok((записи, предупреждения)) или
/// Err(причина для warnings — анонсы не прочитаны).
pub async fn items(
    ctx: &Ctx,
    known: &HashMap<String, String>,
    quote: &str,
) -> Result<(Vec<DelistItem>, Vec<String>), String> {
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
    let fetch = Fetch {
        http: &ctx.http,
        limits: ctx.limits(),
        per_refresh: ctx.int("delist.ann_html_per_refresh"),
    };
    let k = Known { map: known, quote };
    Ok(delist_ann::collect(&anns, &k, d.since, SOURCE, &fetch).await)
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
        let other = a.get("tags").and_then(Value::as_array).is_some_and(|t| {
            t.iter()
                .any(|x| x.as_str().is_some_and(|s| NOT_SPOT.contains(&s)))
        });
        if key != Some(ANN_TYPE) || other || !crate::gateway::delist_html::spot(&title) {
            continue;
        }
        if !title.to_ascii_lowercase().contains(MARK) {
            continue;
        }
        // Перечень монет — только в заголовке «Delisting of …»; у остальных — на странице анонса.
        let list = title
            .strip_prefix(HEAD_LIST)
            .map(|rest| rest.split(DATE_SEP).next().unwrap_or(rest).to_string());
        out.push(Ann {
            list,
            title: title.clone(),
            desc: num::text_opt(a, "description").unwrap_or_default(),
            url: num::text_opt(a, "url").filter(|u| !u.is_empty()),
            at,
        });
    }
    Ok((out, oldest))
}
