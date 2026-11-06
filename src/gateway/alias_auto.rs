//! Файл: gateway/alias_auto.rs
//! Что делает: решение 206 — автоматическая склейка ВСЕХ тикеров одного адреса контракта BSC (coin.ticker_alias_auto).
//! Общая книга шлюза: какие тикеры у каждого адреса в живых справочниках сетей бирж (каждая обёртка биржи пишет свой
//! справочник при каждом coin_networks, gateway/aliased.rs). Адрес, у которого биржи зовут монету по-разному, — одна
//! монета; основной тикер выбирается правилом pick_main и ЗАКРЕПЛЯЕТСЯ до перезапуска (пока он среди тикеров адреса),
//! чтобы ядро не видело смены имени монеты, когда справочники бирж приходят по очереди.
//! Ручной coin.ticker_alias сильнее: адрес из него автоматически не склеивается. Адрес из coin.ticker_alias_off — не
//! склеивается вовсе («Разделить» на экране «Монеты»). Конфликт (у биржи основной тикер занят другой монетой) решает
//! alias_map::venue_map — эта биржа не склеивается, журнал.
//! Кто вызывает: gateway/aliased.rs (record, groups), gateway/registry.rs (общая книга), api/coins.rs (view).
//! От чего зависит: alias_map.rs (Group), core/types.
//! Настройки: нет (coin.ticker_alias_auto, coin.ticker_alias_off читает aliased.rs).

use std::collections::{BTreeMap, HashMap};

use alloy_primitives::Address;
use serde::Serialize;

use crate::core::types::CoinNetworkInfo;

use super::alias_map::Group;

/// Книга тикеров по адресам.
#[derive(Debug, Default)]
pub struct AutoBook {
    /// Адрес → (биржа → тикер биржи). BTreeMap — порядок бирж стабилен.
    seen: HashMap<Address, BTreeMap<String, String>>,
    /// Закреплённый основной тикер адреса.
    main: HashMap<Address, String>,
}

/// Строка экрана: адрес, основной тикер, тикеры бирж.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutoRow {
    pub address: String,
    pub main: String,
    pub venues: Vec<(String, String)>,
}

impl AutoBook {
    /// Записывает справочник биржи (тикеры адресов сети BSC; прежние записи этой биржи заменяются). Ошибок нет.
    pub fn record(&mut self, venue: &str, raw: &[CoinNetworkInfo]) {
        for m in self.seen.values_mut() {
            m.remove(venue);
        }
        for i in raw {
            if let Some(a) = i.asset.bsc_contract {
                self.seen
                    .entry(a)
                    .or_default()
                    .insert(venue.to_string(), i.asset.ticker.clone());
            }
        }
        self.seen.retain(|_, m| !m.is_empty());
    }

    /// Автоматические группы: адреса с разными тикерами у бирж, кроме `skip` (адреса ручной склейки и «Разделить»).
    /// Основной тикер — закреплённый, иначе pick_main (и закрепляется). Ошибок нет.
    pub fn groups(&mut self, skip: &[Address]) -> Vec<Group> {
        let mut out = Vec::new();
        for (a, m) in &self.seen {
            if skip.contains(a) {
                continue;
            }
            let tickers = distinct(m.values());
            if tickers.len() < 2 {
                continue;
            }
            let counts: Vec<(String, usize)> = tickers
                .iter()
                .map(|t| {
                    let n = m.values().filter(|x| x.eq_ignore_ascii_case(t)).count();
                    (t.clone(), n)
                })
                .collect();
            let kept = self
                .main
                .get(a)
                .filter(|k| tickers.iter().any(|t| t.eq_ignore_ascii_case(k)))
                .cloned();
            let main = kept.unwrap_or_else(|| pick_main(&counts));
            self.main.insert(*a, main.clone());
            let mut list = vec![main.clone()];
            list.extend(
                tickers
                    .into_iter()
                    .filter(|t| !t.eq_ignore_ascii_case(&main)),
            );
            out.push(Group {
                address: *a,
                tickers: list,
            });
        }
        out.sort_by_key(|g| g.address);
        out
    }

    /// Строки экрана для групп. Ошибок нет.
    pub fn view(&self, groups: &[Group]) -> Vec<AutoRow> {
        groups
            .iter()
            .map(|g| AutoRow {
                address: g.address.to_string(),
                main: g.tickers[0].clone(),
                venues: self
                    .seen
                    .get(&g.address)
                    .map(|m| m.iter().map(|(v, t)| (v.clone(), t.clone())).collect())
                    .unwrap_or_default(),
            })
            .collect()
    }
}

/// Разные тикеры без учёта регистра, по порядку первого появления. Ошибок нет.
fn distinct<'a>(it: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for t in it {
        if !v.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            v.push(t.clone());
        }
    }
    v
}

/// Правило основного тикера (решение 206): 1) у большего числа бирж; 2) без «OLD» в имени; 3) не кончается цифрой;
/// 4) короче; 5) по алфавиту. Принимает: (тикер, сколько бирж). Возвращает: тикер. Пусто — пустая строка.
pub fn pick_main(counts: &[(String, usize)]) -> String {
    let worse = |t: &str| {
        let u = t.to_uppercase();
        (
            u.contains("OLD") as u8,
            u.chars().last().is_some_and(|c| c.is_ascii_digit()) as u8,
        )
    };
    counts
        .iter()
        .min_by(|(a, na), (b, nb)| {
            nb.cmp(na)
                .then(worse(a).cmp(&worse(b)))
                .then(a.len().cmp(&b.len()))
                .then(a.cmp(b))
        })
        .map(|(t, _)| t.clone())
        .unwrap_or_default()
}
