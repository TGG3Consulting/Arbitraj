//! Файл: core/net_chain.rs
//! Что делает: решение 431 (шаг 3 двух сетей) — привязка записей справочника сетей биржи к сети ядра. Адаптер биржи
//! ставит адрес контракта у каждой своей сети (gateway/own_nets.rs: venue.bsc_network и venue.<сеть>_network), а к какой
//! сети ядра относится запись, решает только ядро: запись сети BSC (venue.bsc_network) — без сети, как раньше; запись
//! сети `c`, у которой в боте есть площадка DEX (dex.chain, не сеть по умолчанию), — `chain = Some(c)` (канонический вид
//! core/chain.rs); запись своей сети, у которой площадки DEX нет, — без адреса (как до решения 431: адрес был только у
//! BSC). Так справочник BSC остаётся байт в байт прежним, даже если имя сети Arbitrum у биржи уже задано, а адреса
//! разных сетей не склеиваются (один адрес в BSC и в Arbitrum — разные монеты).
//! Решение 489: «площадка DEX этой сети в боте есть» = площадка DEX В ИГРЕ (core/chain_play.rs active_dexes: зарегистрирована и
//! venue.enabled = да или её держат идущие сделки — решение 196) — то же понятие, что у групп справочника
//! (registry_chains::groups). Выключенная площадка без сделок сети не даёт: адрес записи её сети снимается (инцидент
//! 21.11.2026 — арбитрумные адреса без сети попали в подписку DEX BSC). Изменение venue.enabled площадки DEX, dex.chain,
//! dex.default_chain (affects, rebuild.rs watch_settings) и отпускание сети сделками (feed_hold::on_saved) — толчок
//! перечитывания справочника сетей бирж (привязка пишется при получении; толчок внутри окна venue.networks_fresh_ms ждёт
//! его конца — registry_sync.rs fresh_left).
//! Кто вызывает: core/registry_sync.rs (справочник биржи), core/precheck.rs (справочник одной монеты),
//! core/deposit_min_ask.rs (минимум депозита) — перед native::map (prepare); affects — core/rebuild.rs (watch_settings).
//! От чего зависит: chain.rs, chain_play.rs, native.rs, gateway/own_nets.rs, settings, engine.rs.
//! Настройки: venue.bsc_network, venue.arbitrum_network (по бирже); dex.chain, dex.default_chain, venue.enabled (площадки
//! DEX; через chain.rs).

use crate::core::types::{ChainId, CoinNetworkInfo, VenueId};
use crate::gateway::own_nets;
use crate::settings::live::Change;
use crate::settings::read::At;
use crate::settings::scope::Scope;

use super::engine::Core;

/// Что делает: своя сеть биржи не BSC — (имя сети у биржи, сеть ядра или None — площадки DEX этой сети в игре нет,
/// решение 489). Принимает: ядро, биржу. Возвращает: список (пусто — других своих сетей у биржи не задано). Ошибок нет:
/// площадка DEX, чью сеть не прочитать, и выключенная площадка без идущих сделок не считаются.
pub fn others(core: &Core, v: &VenueId) -> Vec<(String, Option<ChainId>)> {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    let names: Vec<(&str, String)> = own_nets::OTHER
        .iter()
        .filter_map(|o| s.text(o, at).ok().map(|n| (*o, n)))
        .filter(|(_, n)| !n.is_empty())
        .collect();
    if names.is_empty() {
        return vec![];
    }
    // Решение 489: сети — только площадок DEX в игре (то же понятие, что у сборки справочника).
    let tags: Vec<ChainId> = super::chain_play::active_dexes(core)
        .iter()
        .filter_map(|d| super::chain::tag_of_dex(core, d).ok().flatten())
        .collect();
    names
        .into_iter()
        .map(|(o, n)| {
            let c = tags
                .iter()
                .find(|c| own_nets::setting_of(Some(*c)) == o)
                .cloned();
            (n, c)
        })
        .collect()
}

/// Что делает: привязка записей к сети ядра (шапка). Других своих сетей у биржи нет — список как есть (поведение до
/// решения 431). Принимает: ядро, биржу, справочник из её API. Возвращает: список. Ошибок нет.
pub fn tag(core: &Core, v: &VenueId, mut list: Vec<CoinNetworkInfo>) -> Vec<CoinNetworkInfo> {
    let others = others(core, v);
    if others.is_empty() {
        return list;
    }
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    let bsc = core.inner.deps.settings.text(own_nets::BSC, at).ok();
    for i in &mut list {
        // Имя сети BSC совпало с именем другой сети (ошибка настройки) — запись остаётся сетью BSC, как раньше.
        if bsc.as_deref() == Some(i.network.as_str()) {
            continue;
        }
        match others.iter().find(|(n, _)| *n == i.network) {
            Some((_, Some(c))) => i.asset.chain = Some(c.clone()),
            Some((_, None)) => i.asset.contract = None,
            None => {}
        }
    }
    list
}

/// Что делает: справочник биржи для ядра — привязка к сети (tag) и нативная монета (native::map, решение 62).
/// Принимает: ядро, биржу, справочник из её API. Возвращает: список. Ошибок нет.
pub fn prepare(core: &Core, v: &VenueId, list: Vec<CoinNetworkInfo>) -> Vec<CoinNetworkInfo> {
    super::native::map(core, v, tag(core, v, list))
}

/// Что делает: решение 489 — меняет ли настройка привязку записей к сети ядра (тогда толчок перечитывания справочника
/// сетей бирж): venue.enabled площадки DEX (или всей системы), dex.chain, dex.default_chain. Принимает: ядро, изменение.
/// Возвращает: да/нет. Ошибок нет.
pub fn affects(core: &Core, c: &Change) -> bool {
    match c.name.as_str() {
        "dex.chain" | super::chain::DEFAULT => true,
        "venue.enabled" => match &c.scope {
            Scope::Venue(id) => super::chain::dexes(core).iter().any(|v| v.0 == *id),
            Scope::Global => true,
            _ => false,
        },
        _ => false,
    }
}
