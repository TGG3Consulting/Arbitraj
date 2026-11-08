//! Файл: gateway/delist_ann.rs
//! Что делает: общий разбор анонсов делистинга бирж (решение 226, Тигран 08.11.2026: «делист — наш главный враг»).
//! Адаптер сам запрашивает анонсы своей биржи по её официальному API, сам отбирает только спотовые анонсы типа
//! «delistings» и вырезает из заголовка часть со списком монет («DORA, ICX, STORJ, ZEUS and ELF»); здесь — общие
//! шаги: список → тикеры (строго: каждый элемент — тикер из A–Z и 0–9, иначе анонс не разбирается целиком),
//! отсев тикеров, которых нет в справочнике пар этой биржи с котируемой монетой venue.quote_asset, запись
//! делистинга БЕЗ времени (ядро такую пару на этой бирже не торгует вообще — «время не сообщено», решение 175) и
//! чтение настроек глубины анонсов.
//! Время делистинга из текста анонса не берётся: в API анонсов оно только текстом — блок действует с момента анонса,
//! пока анонс моложе delist.ann_max_age_h (пара после делистинга исчезает из справочника биржи).
//! Кто вызывает: venues/{okx,kucoin,bybit}/delist.rs, tests/delist_sources.rs.
//! От чего зависит: core/types (DelistItem, Delistings), gateway/errors.
//! Настройки: delist.ann_max_age_h, delist.ann_max_pages (читает адаптер через свой ctx; здесь — проверка значений).

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayError;

/// Один анонс делистинга, уже отобранный адаптером (тип delistings, спот).
#[derive(Debug, Clone)]
pub struct Ann {
    /// Часть заголовка со списком монет, например «DORA, ICX, STORJ, ZEUS and ELF».
    pub list: String,
    /// Заголовок целиком (для человека).
    pub title: String,
    /// Время публикации анонса.
    pub at: DateTime<Utc>,
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

/// Что делает: тикеры из списка заголовка. Разделители — «, » и « and » (в том числе «, and»). Принимает: список.
/// Возвращает: Some(тикеры) — каждый элемент списка похож на тикер (A–Z, 0–9, хотя бы одна буква); None — хоть один
/// элемент не тикер («selected USD», «BTC/BRZ», «leveraged token pairs») — анонс не разбирается целиком.
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

/// Что делает: записи делистинга по анонсам. Принимает: анонсы, справочник биржи (тикер → символ пары с котируемой
/// монетой venue.quote_asset), глубину (старые анонсы не учитываются), имя источника для человека («OKX GET
/// /api/v5/support/announcements»). Возвращает: записи без времени — только тикеры, которые есть в справочнике.
/// Ошибок нет.
pub fn items(
    anns: &[Ann],
    known: &HashMap<String, String>,
    since: DateTime<Utc>,
    source: &str,
) -> Vec<DelistItem> {
    let mut out = vec![];
    for a in anns.iter().filter(|a| a.at >= since) {
        let Some(list) = tickers(&a.list) else {
            continue;
        };
        for t in list {
            let Some(symbol) = known.get(&t) else {
                continue;
            };
            out.push(DelistItem {
                ticker: t,
                symbol: symbol.clone(),
                trading_end: None,
                withdraw_end: None,
                source: format!(
                    "{source}: анонс {} «{}» — блок с момента анонса",
                    a.at.format("%d.%m.%Y"),
                    a.title
                ),
            });
        }
    }
    out
}

/// Что делает: добавляет к ответу основного источника записи анонсов или причину, почему они не прочитаны.
/// Принимает: ответ основного источника, итог анонсов. Возвращает: общий ответ. Ошибок нет.
pub fn merge(mut base: Delistings, ann: Result<Vec<DelistItem>, String>) -> Delistings {
    match ann {
        Ok(items) => base.items.extend(items),
        Err(why) => base.warnings.push(why),
    }
    base
}
