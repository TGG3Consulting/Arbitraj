//! Файл: gateway/alias_map.rs
//! Что делает: решение 200 — склейка тикеров одного адреса контракта (coin.ticker_alias). Чистая логика без ожиданий:
//! разбор настройки в группы «адрес → тикеры», карта переименований одной биржи по её живому справочнику сетей
//! (тикер биржи ↔ основной тикер) и перевод тикеров туда и обратно. Ядро видит только основной тикер (одна монета —
//! одна запись справочника, направления между биржами есть); биржа в запросах — только свой тикер (gateway/aliased.rs).
//! Защита решения 118 остаётся: склеиваются ТОЛЬКО тикеры, у которых в справочнике биржи тот же адрес контракта в BSC;
//! у биржи есть другая монета с основным тикером — склейки у этой биржи нет (конфликт в журнал).
//! Кто вызывает: gateway/aliased.rs, tests.
//! От чего зависит: core/types (CoinNetworkInfo).
//! Настройки: coin.ticker_alias (значение передаёт вызывающий).
//!
//! Формат элемента coin.ticker_alias: «<адрес контракта BSC>=<ОСНОВНОЙ>,<ДРУГОЙ>[,…]», например
//! «0x44Ece1031e5b5e2D9169546cC10EA5C95BA96237=ATEAM,AMAZINGTEAM». Тикеры сравниваются без учёта регистра.

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::core::types::CoinNetworkInfo;

/// Группа склейки: адрес и тикеры (первый — основной, его видит ядро).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub address: Address,
    pub tickers: Vec<String>,
}

/// Разбирает coin.ticker_alias. Принимает: элементы списка. Возвращает: группы и причины отказа по неверным
/// элементам (они не применяются). Ошибок нет.
pub fn parse(items: &[String]) -> (Vec<Group>, Vec<String>) {
    let (mut groups, mut bad) = (Vec::new(), Vec::new());
    for raw in items {
        match parse_one(raw) {
            Ok(g) if groups.iter().any(|x: &Group| x.address == g.address) => {
                bad.push(format!(
                    "«{raw}»: адрес {} уже есть в другом элементе",
                    g.address
                ));
            }
            Ok(g) => groups.push(g),
            Err(why) => bad.push(format!("«{raw}»: {why}")),
        }
    }
    (groups, bad)
}

/// Один элемент. Ошибка — текст причины.
fn parse_one(raw: &str) -> Result<Group, String> {
    let (addr, list) = raw
        .split_once('=')
        .ok_or("нужен вид «адрес=ОСНОВНОЙ,ДРУГОЙ»")?;
    let address: Address = addr
        .trim()
        .parse()
        .map_err(|_| format!("«{}» — не адрес контракта", addr.trim()))?;
    let mut tickers: Vec<String> = Vec::new();
    for t in list.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !tickers.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            tickers.push(t.to_string());
        }
    }
    if tickers.len() < 2 {
        return Err("нужно хотя бы два разных тикера".into());
    }
    Ok(Group { address, tickers })
}

/// Переименование одной биржи: тикер биржи → основной тикер монеты с этим адресом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub address: Address,
    pub main: String,
    pub venue: String,
}

/// Карта переименований одной биржи.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VenueMap {
    pub renames: Vec<Rename>,
}

impl VenueMap {
    /// Тикер для запроса к бирже: основной тикер склеенной монеты → тикер биржи; адрес задан и другой — без перевода
    /// (другая монета с тем же тикером). Ошибок нет.
    pub fn out(&self, ticker: &str, address: Option<Address>) -> Option<&str> {
        self.renames
            .iter()
            .find(|r| r.main == ticker && address.is_none_or(|a| a == r.address))
            .map(|r| r.venue.as_str())
    }

    /// Тикер для ядра: тикер биржи → основной. Ошибок нет.
    pub fn back(&self, ticker: &str) -> Option<&str> {
        self.renames
            .iter()
            .find(|r| r.venue == ticker)
            .map(|r| r.main.as_str())
    }

    /// Пустая — переводить нечего. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.renames.is_empty()
    }
}

/// Карта одной биржи по её живому справочнику сетей. Принимает: группы, справочник биржи (тикеры биржи).
/// Возвращает: карту и конфликты (текстом, для журнала). Ошибок нет.
pub fn venue_map(groups: &[Group], raw: &[CoinNetworkInfo]) -> (VenueMap, Vec<String>) {
    let (mut map, mut conflicts) = (VenueMap::default(), Vec::new());
    for g in groups {
        let main = &g.tickers[0];
        let Some(own) = raw.iter().find(|i| {
            i.asset.bsc_contract == Some(g.address)
                && g.tickers
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&i.asset.ticker))
        }) else {
            continue;
        };
        let venue = own.asset.ticker.clone();
        if venue == *main {
            continue;
        }
        if raw
            .iter()
            .any(|i| i.asset.ticker.eq_ignore_ascii_case(main))
        {
            conflicts.push(format!(
                "у биржи уже есть монета {main} (другая) — {venue} с адресом {} не склеен",
                g.address
            ));
            continue;
        }
        map.renames.push(Rename {
            address: g.address,
            main: main.clone(),
            venue,
        });
    }
    (map, conflicts)
}

/// Карты всех бирж по id — общий тип состояния склейки шлюза.
pub type Maps = HashMap<String, VenueMap>;
