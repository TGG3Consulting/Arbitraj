//! Файл: venues/bitget/delist_ann.rs
//! Что делает: анонсы делистинга Bitget (решение 226, уточнение Тиграна 08.11.2026: «непокрытого быть не должно») —
//! страницы GET /api/v2/public/annoucements с annType symbol_delisting на английском, не старше
//! delist.ann_max_age_h (startTime); отбор спотовых анонсов делистинга: в заголовке «Delist», нет слов про ботов,
//! маржу, займы, фьючерсы (gateway/delist_html::spot) — «Notice of Delisting 2 Spot Trading Pairs on November 13,
//! 2026». Тикеров в заголовке у Bitget нет, annDesc «Deprecated» (живой ответ — просто «symbol_delisting») — пары
//! и время берутся со страницы annUrl: «Bitget will delist the following trading pairs from the unified account
//! (spot trading) on November 13, 2026, 10:00 AM (UTC): ALPINE/USDT; QUBIC/USDT» (gateway/delist_ann.rs).
//! Кто вызывает: delist.rs, tests/delist_ann.rs (parse).
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, gateway/{delist_ann,delist_page,delist_html}.rs.
//! Настройки: delist.ann_max_age_h, delist.ann_max_pages, delist.ann_html_per_refresh, http.* (страницы анонсов).
//!
//! Официальная документация: Get All Notices GET /api/v2/public/annoucements
//! https://www.bitget.com/api-doc/classic/common/notice/Get-All-Notices — «Frequency limit: 20 times/1s (IP)»;
//! annType (… symbol_delisting), startTime / endTime (мс, по времени анонса), cursor («pass the last annId in the
//! previous response»), limit («By default, it is 10 entries, and the maximum is 10 entries»), language (en_US);
//! поиск — только за последний месяц; ответ data = [{annId, annTitle, annDesc (Deprecated), annType, annSubType,
//! language, annUrl, cTime}]. Живой ответ 08.11.2026: 4 анонса за месяц (2 спотовых делистинга, удаление пар из
//! ботов, отключение маржи) — одной страницы хватает.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::DelistItem;
use crate::gateway::delist_ann::{self, Ann, Known};
use crate::gateway::delist_page::Fetch;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Путь (так в документации Bitget — «annoucements»), тип анонсов и язык — значения документации.
const PATH: &str = "/api/v2/public/annoucements";
const ANN_TYPE: &str = "symbol_delisting";
const LANG: &str = "en_US";
/// Источник для человека.
const SOURCE: &str = "Bitget GET /api/v2/public/annoucements";
/// Признак делистинга в заголовке (формат живых анонсов Bitget, без учёта регистра).
const MARK: &str = "delist";

/// Что делает: читает страницы анонсов (курсор — последний annId) и страницы самих анонсов, строит записи.
/// Принимает: окружение, справочник «тикер → символ», котируемую монету. Возвращает: Ok((записи, предупреждения))
/// или Err(причина для warnings — анонсы не прочитаны).
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
    let (mut anns, mut cursor) = (vec![], None::<String>);
    for _ in 0..d.max_pages {
        let mut q = vec![
            ("annType", ANN_TYPE.to_string()),
            ("language", LANG.to_string()),
            ("startTime", d.since.timestamp_millis().to_string()),
        ];
        if let Some(c) = &cursor {
            q.push(("cursor", c.clone()));
        }
        let data = super::rest::public(ctx, PATH, q)
            .await
            .map_err(|e| format!("анонсы Bitget: {}", e.message))?;
        let (got, last) = parse(&data).map_err(|e| format!("анонсы Bitget: {}", e.message))?;
        anns.extend(got);
        match last {
            Some(id) if cursor.as_deref() != Some(id.as_str()) => cursor = Some(id),
            _ => break,
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

/// Что делает: разбор data одной страницы. Принимает: data (массив анонсов). Возвращает: (спотовые анонсы
/// делистинга, annId последнего анонса страницы — курсор следующей; None — страница пуста). Ошибка BadResponse —
/// data не массив, у анонса нет annId, annTitle или cTime.
pub fn parse(data: &Value) -> GatewayResult<(Vec<Ann>, Option<String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("annoucements: data не массив", &data.to_string()))?;
    let (mut out, mut last) = (vec![], None);
    for a in list {
        last = Some(num::text(a, "annId")?);
        let title = num::text(a, "annTitle")?;
        let ms: i64 = num::text(a, "cTime")?
            .parse()
            .map_err(|_| super::errors::bad("cTime не число", &a.to_string()))?;
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("cTime вне диапазона", &a.to_string()))?;
        let delist = title.to_ascii_lowercase().contains(MARK);
        if num::text_opt(a, "annType").as_deref() != Some(ANN_TYPE)
            || !delist
            || !crate::gateway::delist_html::spot(&title)
        {
            continue;
        }
        out.push(Ann {
            list: None,
            title: title.clone(),
            desc: String::new(),
            url: num::text_opt(a, "annUrl").filter(|u| !u.is_empty()),
            at,
        });
    }
    Ok((out, last))
}
