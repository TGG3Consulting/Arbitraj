//! Файл: core/coin_alias_sync.rs
//! Что делает: решение 211 (ревью В9) — сообщает хранилищу настроек тикеры каждой монеты с адресом (показной тикер и
//! тикеры площадок) для запасного чтения области «монета» по старому тикеру (settings/coin_alias.rs).
//! Кто вызывает: core/rebuild.rs (после сборки справочника).
//! От чего зависит: registry.rs (CoinEntry), settings (store, scope).
//! Настройки: нет.

use std::collections::HashMap;

use super::registry::CoinEntry;

/// Адрес «0x…» → тикеры монеты (показной первым, затем тикеры площадок, без повторов). Монеты без адреса не участвуют
/// (их область и есть тикер). Принимает: монеты справочника. Ошибок нет.
pub fn aliases<'a>(coins: impl Iterator<Item = &'a CoinEntry>) -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    for c in coins {
        let Some(a) = c.asset.bsc_contract else {
            continue;
        };
        let list = m
            .entry(crate::settings::scope::coin_id(&c.asset.ticker, Some(&a)))
            .or_default();
        let names = std::iter::once(&c.asset.ticker).chain(c.presence.iter().map(|p| &p.ticker));
        for t in names {
            if !list.contains(t) {
                list.push(t.clone());
            }
        }
    }
    m
}
