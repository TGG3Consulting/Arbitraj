//! Файл: core/networks_merge.rs
//! Что делает: записи сетей ОДНОЙ монеты биржи (ответ coin_networks_of) — в справочник сетей ядра вместо её старых
//! записей; остальные монеты биржи не трогаются (решение 117: ответ одной монеты целиком заменял справочник биржи —
//! остальные монеты теряли сеть BSC до следующего опроса). Справочника биржи ещё нет — не пишем (его соберёт
//! registry_sync.rs). После записи — толчок пересборки справочника монет.
//! Решение 387: вынесено из precheck.rs (transfer_open) без изменения смысла — тем же кодом пишет и запрос минимума
//! депозита по требованию расчёта (deposit_min_ask.rs).
//! Кто вызывает: core/precheck.rs (transfer_open), core/deposit_min_ask.rs.
//! От чего зависит: engine.rs (справочник сетей ядра, пересборка), core/types.
//! Настройки: нет.

use std::collections::HashSet;
use std::sync::Arc;

use crate::core::types::{CoinNetworkInfo, VenueId};

use super::engine::{Core, wr};

/// Что делает: заменяет в справочнике сетей ядра записи монеты `on` биржи `v` свежими `list` и толкает пересборку.
/// Решение 211: записи этой монеты площадки — по её тикеру (и именам из свежего ответа после native::map); показное
/// имя ядра у этой биржи может быть другой монетой — по нему не чистим.
/// Принимает: ядро, биржу, тикер монеты на бирже, свежие записи (уже после native::map). Ошибок нет.
pub fn merge_coin(core: &Core, v: &VenueId, on: &str, list: Vec<CoinNetworkInfo>) {
    {
        let mut nets = wr(&core.inner.networks);
        if let Some(all) = nets.get_mut(v).map(Arc::make_mut) {
            let mut gone: HashSet<String> = list.iter().map(|i| i.asset.ticker.clone()).collect();
            gone.insert(on.to_string());
            all.retain(|i| !gone.contains(&i.asset.ticker));
            all.extend(list);
        }
    }
    core.request_rebuild();
}
