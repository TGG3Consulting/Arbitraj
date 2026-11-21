//! Файл: gateway/delist_ann.rs
//! Что делает: общий разбор анонсов делистинга бирж (решение 226; уточнение Тиграна 08.11.2026: «любая монета,
//! делистинг которой на бирже X наступит меньше чем через N часов, на этой бирже больше не торгуется» — единое
//! правило delist.block_hours для всех источников, включая анонсы). Адаптер сам запрашивает анонсы по официальному
//! API своей биржи и отбирает только спотовые анонсы делистинга (заголовок, описание, url, список монет из
//! заголовка, если он там есть); здесь — общие шаги:
//! (1) тикеры: перечень в заголовке («DORA, ICX, STORJ, ZEUS and ELF» — строго, каждый элемент A–Z/0–9), иначе — со
//!     страницы анонса (gateway/delist_html.rs); только тикеры справочника биржи с котируемой монетой
//!     venue.quote_asset;
//! (2) время делистинга: из заголовка и описания, которые отдаёт API (gateway/delist_time.rs: ровно один момент
//!     «дата + время + UTC»), иначе — со страницы анонса; не определено однозначно — запись БЕЗ времени (ядро такую
//!     монету на этой бирже не торгует, решение 175) с причиной «время делистинга из анонса не определено»;
//! (3) страница читается только если API не дал тикеры или время (gateway/delist_page.rs: кеш по url, лимит).
//! Ядро блокирует монету за delist.block_hours до времени делистинга (core/delist_check.rs), как у всех источников.
//! Кто вызывает: venues/{okx,kucoin,bybit,bitget}/delist_ann.rs, tests/delist_ann.rs, tests/delist_sources.rs.
//! От чего зависит: core/types (DelistItem, Delistings), gateway/errors, delist_time.rs, delist_html.rs,
//! delist_page.rs.
//! Настройки: delist.ann_max_age_h, delist.ann_max_pages, delist.ann_html_per_refresh (читает адаптер через свой
//! ctx; здесь — проверка значений).

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayError;

use super::{delist_html, delist_page, delist_time};

/// Один анонс делистинга, уже отобранный адаптером (спот, делистинг).
#[derive(Debug, Clone)]
pub struct Ann {
    /// Часть заголовка со списком монет («DORA, ICX, STORJ, ZEUS and ELF»); None — списка в заголовке нет
    /// («Multiple Tokens», «Bybit to Delist 1 Token(s)…», «Notice of Delisting 2 Spot Trading Pairs…»).
    pub list: Option<String>,
    /// Заголовок целиком (для человека и поиска времени).
    pub title: String,
    /// Описание из API (KuCoin annDesc, Bybit description); пусто — API описания не отдаёт.
    pub desc: String,
    /// Официальная страница анонса из API (annUrl, url); None — API ссылки не отдаёт.
    pub url: Option<String>,
    /// Время публикации анонса.
    pub at: DateTime<Utc>,
}

/// Справочник биржи для сверки: тикер → символ пары с котируемой монетой quote.
pub struct Known<'a> {
    pub map: &'a HashMap<String, String>,
    pub quote: &'a str,
}

/// Глубина чтения анонсов: не старше max_age, не больше max_pages страниц.
#[derive(Debug, Clone, Copy)]
pub struct Depth {
    pub since: DateTime<Utc>,
    pub max_pages: u64,
}

/// Что делает: глубина анонсов из настроек. Принимает: результаты чтения delist.ann_max_age_h и
/// delist.ann_max_pages (ctx.int адаптера), «сейчас». Возвращает: Depth. Ошибки: текст для warnings — какая настройка
/// не задана (анонсы тогда не читаются, основной источник действует).
pub fn depth(
    age_h: Result<u64, GatewayError>,
    pages: Result<u64, GatewayError>,
    now: DateTime<Utc>,
) -> Result<Depth, String> {
    let age_h = age_h.map_err(|e| format!("анонсы делистинга не читаются: {}", e.message))?;
    let max_pages = pages.map_err(|e| format!("анонсы делистинга не читаются: {}", e.message))?;
    let hours =
        i64::try_from(age_h).map_err(|_| "delist.ann_max_age_h слишком велика".to_string())?;
    let since = now
        .checked_sub_signed(Duration::hours(hours))
        .ok_or_else(|| "delist.ann_max_age_h слишком велика".to_string())?;
    Ok(Depth { since, max_pages })
}

/// Что делает: тикеры из списка заголовка. Разделители — «,» и « and » (в том числе «, and»). Принимает: список.
/// Возвращает: Some(тикеры) — каждый элемент списка похож на тикер (A–Z, 0–9, хотя бы одна буква); None — хоть один
/// элемент не тикер («Multiple Tokens», «BTC/BRZ», «leveraged token pairs») — тикеры ищутся на странице анонса.
pub fn tickers(list: &str) -> Option<Vec<String>> {
    let mut out = vec![];
    for part in list.split(',') {
        for word in part.split(" and ") {
            let w = word.trim();
            if w.is_empty() {
                continue;
            }
            let ok = w
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && w.chars().any(|c| c.is_ascii_uppercase());
            if !ok {
                return None;
            }
            out.push(w.to_string());
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Что делает: время делистинга из текста API анонса (заголовок + описание). Принимает: анонс. Возвращает: момент,
/// если в тексте ровно один «дата + время + UTC». Ошибок нет.
pub fn api_time(a: &Ann) -> Option<DateTime<Utc>> {
    delist_time::one(&format!("{}\n{}", a.title, a.desc))
}

/// Что делает: нужна ли анонсу страница. Принимает: анонс. Возвращает: true — в API нет перечня тикеров или
/// времени, а ссылка на страницу есть. Ошибок нет.
pub fn needs_page(a: &Ann) -> bool {
    let no_list = a.list.as_deref().and_then(tickers).is_none();
    a.url.is_some() && (no_list || api_time(a).is_none())
}

/// Что делает: записи делистинга по анонсам. Принимает: анонсы, прочитанные страницы (url → строки), справочник,
/// начало глубины (старые анонсы не учитываются), имя источника для человека. Возвращает: (записи — только тикеры
/// справочника; предупреждения — анонсы, у которых не удалось узнать тикеры, для warnings). Ошибок нет.
pub fn items(
    anns: &[Ann],
    pages: &HashMap<String, Vec<String>>,
    missed: &HashMap<String, String>,
    known: &Known,
    since: DateTime<Utc>,
    source: &str,
) -> (Vec<DelistItem>, Vec<String>) {
    let (mut out, mut warn) = (vec![], vec![]);
    for a in anns.iter().filter(|a| a.at >= since) {
        let page = a.url.as_ref().and_then(|u| pages.get(u));
        let from_page = page.map(|l| delist_html::pairs(l, known.map, known.quote));
        let api_at = api_time(a);
        let list: Vec<String> = match a.list.as_deref().and_then(tickers) {
            Some(t) => t
                .into_iter()
                .filter(|t| known.map.contains_key(t))
                .collect(),
            None => match &from_page {
                Some(p) => p.keys().cloned().collect(),
                None => {
                    let why = match &a.url {
                        None => "у анонса нет ссылки на страницу".to_string(),
                        Some(u) => missed
                            .get(u)
                            .cloned()
                            .unwrap_or_else(|| "страница анонса не прочитана".into()),
                    };
                    warn.push(format!("анонс «{}»: монеты не определены — {why}", a.title));
                    continue;
                }
            },
        };
        for t in list {
            let page_at = from_page
                .as_ref()
                .and_then(|p| p.get(&t).copied().flatten());
            let (at, how) = match (api_at, page_at) {
                (Some(x), _) => (Some(x), "время из текста API анонса"),
                (None, Some(x)) => (Some(x), "время со страницы анонса"),
                (None, None) => (None, "время делистинга из анонса не определено"),
            };
            let Some(symbol) = known.map.get(&t) else {
                continue;
            };
            out.push(DelistItem {
                ticker: t.clone(),
                symbol: symbol.clone(),
                trading_end: at,
                withdraw_end: None,
                source: format!(
                    "{source}: анонс {} «{}» — {how}",
                    a.at.format("%d.%m.%Y"),
                    a.title
                ),
                st: false,
            });
        }
    }
    (out, warn)
}

/// Что делает: страницы, которые нужны, читает (кеш, лимит) и строит записи. Принимает: анонсы, справочник,
/// начало глубины, имя источника, как читать страницы. Возвращает: (записи, предупреждения). Ошибок нет.
pub async fn collect(
    anns: &[Ann],
    known: &Known<'_>,
    since: DateTime<Utc>,
    source: &str,
    fetch: &delist_page::Fetch<'_>,
) -> (Vec<DelistItem>, Vec<String>) {
    let mut urls: Vec<String> = vec![];
    for a in anns.iter().filter(|a| a.at >= since && needs_page(a)) {
        if let Some(u) = &a.url
            && !urls.contains(u)
        {
            urls.push(u.clone());
        }
    }
    let (pages, missed) = delist_page::pages(&urls, fetch).await;
    let (items, mut warn) = items(anns, &pages, &missed, known, since, source);
    // Монеты известны, а страница (за временем) не прочитана — запись уже без времени; причина — в warnings.
    for a in anns
        .iter()
        .filter(|a| a.url.as_ref().is_some_and(|u| missed.contains_key(u)))
    {
        if a.list.as_deref().and_then(tickers).is_some() {
            let why = a
                .url
                .as_ref()
                .and_then(|u| missed.get(u))
                .cloned()
                .unwrap_or_default();
            warn.push(format!(
                "анонс «{}»: страница не прочитана — {why}",
                a.title
            ));
        }
    }
    (items, warn)
}

/// Что делает: добавляет к ответу основного источника записи анонсов и предупреждения (или причину, почему анонсы
/// не прочитаны). Принимает: ответ основного источника, итог анонсов. Возвращает: общий ответ. Ошибок нет.
pub fn merge(
    mut base: Delistings,
    ann: Result<(Vec<DelistItem>, Vec<String>), String>,
) -> Delistings {
    match ann {
        Ok((items, warn)) => {
            base.items.extend(items);
            base.warnings.extend(warn);
        }
        Err(why) => base.warnings.push(why),
    }
    base
}
