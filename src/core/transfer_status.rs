//! Файл: core/transfer_status.rs
//! Что делает: смены статусов ввода и вывода по сети BSC между двумя опросами справочника сетей биржи
//! (core/registry_sync.rs) и текст уведомления: по одному на монету или — если за один опрос сменилось не меньше
//! venue.status_bulk_min монет — одна сводка (журнал и Telegram), а полный список — в data записи журнала.
//! Так массовая смена (например ограничение аккаунта на бирже или работы сети) не даёт сотни сообщений.
//! Кто вызывает: core/registry_sync.rs.
//! От чего зависит: core/types.
//! Настройки: venue.status_bulk_min (по площадке; читает registry_sync.rs). Не задана — по сообщению на монету.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::core::types::CoinNetworkInfo;

/// Смена статуса одной монеты: тикер, сеть, ввод открыт, вывод открыт.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub ticker: String,
    pub network: String,
    pub deposit: bool,
    pub withdraw: bool,
}

/// Смены статусов по записям с адресом контракта (сеть BSC). Монеты, которых не было в старом справочнике, —
/// не смена. Принимает: старые статусы (тикер, сеть) → (ввод, вывод), новый справочник. Ошибок нет.
pub fn diff(old: &HashMap<(String, String), (bool, bool)>, new: &[CoinNetworkInfo]) -> Vec<Change> {
    new.iter()
        .filter(|i| i.asset.bsc_contract.is_some())
        .filter_map(|i| {
            let &(dep, wd) = old.get(&(i.asset.ticker.clone(), i.network.clone()))?;
            (dep != i.deposit_enabled || wd != i.withdraw_enabled).then(|| Change {
                ticker: i.asset.ticker.clone(),
                network: i.network.clone(),
                deposit: i.deposit_enabled,
                withdraw: i.withdraw_enabled,
            })
        })
        .collect()
}

/// Слово статуса. Ошибок нет.
fn word(b: bool) -> &'static str {
    if b { "открыт" } else { "закрыт" }
}

/// Текст по одной монете. Ошибок нет.
pub fn one(venue: &str, c: &Change) -> String {
    format!(
        "{venue} {} ({}): ввод {}, вывод {}",
        c.ticker,
        c.network,
        word(c.deposit),
        word(c.withdraw)
    )
}

/// Сводка массовой смены: текст (сколько монет в каждом новом состоянии и тикеры) и полный список для data.
/// Ошибок нет.
pub fn summary(venue: &str, changes: &[Change]) -> (String, Value) {
    let mut groups: std::collections::BTreeMap<(bool, bool), Vec<&str>> = Default::default();
    for c in changes {
        groups
            .entry((c.deposit, c.withdraw))
            .or_default()
            .push(c.ticker.as_str());
    }
    let parts: Vec<String> = groups
        .iter()
        .map(|((d, w), t)| {
            format!(
                "ввод {}, вывод {} — {}: {}",
                word(*d),
                word(*w),
                t.len(),
                t.join(", ")
            )
        })
        .collect();
    let text = format!(
        "{venue}: статус ввода/вывода сменился сразу у {} монет за один опрос. {}",
        changes.len(),
        parts.join("; ")
    );
    let data = json!(changes
        .iter()
        .map(|c| json!({"coin": c.ticker, "network": c.network, "deposit": c.deposit, "withdraw": c.withdraw}))
        .collect::<Vec<_>>());
    (text, data)
}

/// Сводкой ли сообщать: порог задан и смен не меньше него. Ошибок нет.
pub fn bulk(changes: usize, min: Option<u64>) -> bool {
    min.is_some_and(|m| changes as u64 >= m)
}
