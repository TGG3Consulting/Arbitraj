//! Файл: venues/kucoin/delist_ann.rs
//! Что делает: анонсы делистинга KuCoin (решение 226, уточнение Тиграна 08.11.2026) — страницы GET
//! /api/v3/announcements с annType delistings на английском, не старше delist.ann_max_age_h (startTime); отбор только
//! спотовых анонсов делистинга: в заголовке «delist», тип не futures-announcements, нет слов про Earn, фьючерсы,
//! ботов (gateway/delist_html::spot; «KuCoin Earn Will Delist …» не спот). Перечень монет — в заголовке «KuCoin
//! Will Delist A and B on 2026-10-14» (приставка «ST: » отрезается), иначе
//! («Multiple Tokens», «Certain Projects & Their Associated Tokens») — со страницы annUrl (список «Название (ТИКЕР)»);
//! время делистинга — из annDesc («at 8:00 on November 5, 2026 (UTC)»), иначе со страницы (gateway/delist_ann.rs).
//! Кто вызывает: delist.rs, tests/delist_sources.rs, tests/delist_ann.rs (parse).
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, reply.rs (Pool), gateway/{delist_ann,delist_page}.rs.
//! Настройки: delist.ann_max_age_h, delist.ann_max_pages, delist.ann_html_per_refresh, http.* (страницы анонсов).
//!
//! Официальная документация: Get Announcements GET /api/v3/announcements
//! https://www.kucoin.com/docs-new/rest/spot-trading/market-data/get-announcements — пул Public, вес 20; «the default
//! page search is for announcements within a month»; параметры currentPage, pageSize, annType (Allowed values: …
//! delistings …), lang (en_US), startTime («Announcement online start time (milliseconds)»), endTime; ответ data =
//! {totalNum, totalPage, currentPage, pageSize, items: [{annId, annTitle, annType: [..], annDesc, cTime, language,
//! annUrl}]}. Живые ответы 08.11.2026: без pageSize — страница 15 анонсов; за 30 дней 4 анонса типа delistings
//! (3 спотовых), за 90 дней — 19 (11 без фьючерсов, из них 4 — Earn). annDesc спотовых анонсов содержит время:
//! «KuCoin will delist BLAST at 8:00 on October 16, 2026 (UTC).»; «Multiple Tokens» — монеты только на странице
//! (tests/data/delist_ann_2026-11-08/kucoin_page_multiple_tokens_2026-11-05.html).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::DelistItem;
use crate::gateway::delist_ann::{self, Ann, Known};
use crate::gateway::delist_page::Fetch;
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
/// Признак делистинга в заголовке (без учёта регистра), приставка ST-анонсов, начало заголовка с перечнем монет и
/// разделитель даты (формат живых анонсов KuCoin).
const MARK: &str = "delist";
const ST_PREFIX: &str = "ST: ";
const HEAD: &str = "KuCoin Will Delist ";
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
    let fetch = Fetch {
        http: &ctx.http,
        limits: ctx.limits(),
        per_refresh: ctx.int("delist.ann_html_per_refresh"),
    };
    let k = Known { map: known, quote };
    Ok(delist_ann::collect(&anns, &k, d.since, SOURCE, &fetch).await)
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
        if !title.to_ascii_lowercase().contains(MARK) || !crate::gateway::delist_html::spot(&title)
        {
            continue;
        }
        // Перечень монет — только в заголовке «KuCoin Will Delist A and B on …»; у остальных — на странице анонса.
        let bare = title.strip_prefix(ST_PREFIX).unwrap_or(&title);
        let list = bare
            .strip_prefix(HEAD)
            .map(|rest| rest.split(DATE_SEP).next().unwrap_or(rest).to_string());
        out.push(Ann {
            list,
            title: title.clone(),
            desc: num::text_opt(a, "annDesc").unwrap_or_default(),
            url: num::text_opt(a, "annUrl").filter(|u| !u.is_empty()),
            at,
        });
    }
    Ok((out, total))
}
