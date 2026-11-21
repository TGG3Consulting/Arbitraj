//! Файл: tests/t489_common/mod.rs
//! Что делает: решение 489 — общие помощники тестов «DEX в игре»: ядро t198_common (SQLite во временной папке) с биржей
//! bingx (venue.bsc_network = BEP20, venue.arbitrum_network = ARBITRUM — как в инциденте 21.11.2026) и площадками DEX
//! bsc_dex (включена, dex.chain = bsc) и arb_dex (dex.chain = arbitrum; включена — по тесту), dex.default_chain = bsc;
//! справочник сетей BingX из записанного ответа (tests/data/bingx_2026-10-24/config_getall.json, разбор адаптера по
//! своим сетям); справочник пар BingX из записанного symbols.json — событие SpotPairs, как его шлёт адаптер; адреса,
//! которые BingX даёт только в сети ARBITRUM; набор подписки DEX по тому же условию, что core/subscribe.rs wanted.
//! Кто вызывает: tests/core_net_chain_489.rs, tests/core_net_chain_489_bsc.rs.
//! От чего зависит: arb::core (chain, net_chain, subscribe), arb::venues::bingx, arb::gateway::own_nets, t198_common.
//! Моков нет.

#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::Arc;

use alloy_primitives::Address;
use chrono::DateTime;
use serde_json::{Value, json};

use arb::core::Core;
use arb::core::registry::CoinEntry;
use arb::core::types::{ChainId, CoinNetworkInfo, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::own_nets;
use arb::settings::read::At;
use arb::settings::scope::Scope;
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::{symbols, wallet};

#[path = "../t198_common/mod.rs"]
mod t198_common;

/// Время ответа symbols в сыром журнале (мс) — tests/data/bingx_2026-10-24/README.md.
const SYMBOLS_AT_MS: i64 = 1792868538956;

/// Площадка по id. Ошибок нет.
pub fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Сеть Arbitrum ядра (dex.chain площадки arb_dex). Ошибок нет.
pub fn arb_chain() -> ChainId {
    ChainId("arbitrum".into())
}

/// JSON записи BingX. Паника — файла нет.
fn read(name: &str) -> Value {
    let path = format!("tests/data/bingx_2026-10-24/{name}");
    let raw = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// Задаёт настройку. Паника — запись не прошла.
pub async fn set(core: &Core, name: &str, scope: Scope, val: Value) {
    let s = &core.inner.deps.settings;
    s.set(name, scope, Some(val), "тест").await.unwrap();
}

/// Ядро сценария: реестр bingx (биржа), bsc_dex и (если `with_arb`) arb_dex; arb_dex включена, если `arb_on`.
/// Паника — запись настроек не прошла.
pub async fn start(name: &str, with_arb: bool, arb_on: bool) -> Core {
    let core = t198_common::start(name).await;
    let mut kinds = vec![(v("bingx"), VenueKind::Cex)];
    kinds.push((v("bsc_dex"), VenueKind::Dex));
    if with_arb {
        kinds.push((v("arb_dex"), VenueKind::Dex));
    }
    *core.inner.kinds.write().unwrap() = kinds;
    let bx = Scope::Venue("bingx".into());
    for (n, val) in [
        ("venue.enabled", json!(true)),
        ("venue.quote_asset", json!("USDT")),
        ("venue.bsc_network", json!("BEP20")),
        ("venue.arbitrum_network", json!("ARBITRUM")),
        // Записи 2026-10-24: справочник пар должен считаться свежим и сейчас (как tests/core_cold_coins.rs).
        ("venue.symbols_stale_s", json!(1_000_000_000u64)),
    ] {
        set(&core, n, bx.clone(), val).await;
    }
    let bsc = Scope::Venue("bsc_dex".into());
    set(&core, "venue.enabled", bsc.clone(), json!(true)).await;
    if with_arb {
        set(&core, "dex.chain", bsc, json!("bsc")).await;
        let a = Scope::Venue("arb_dex".into());
        set(&core, "dex.chain", a.clone(), json!("arbitrum")).await;
        set(&core, "dex.default_chain", Scope::Global, json!("bsc")).await;
        if arb_on {
            set(&core, "venue.enabled", a, json!(true)).await;
        }
    }
    core
}

/// Справочник сетей BingX по своим сетям из настроек (как адаптер). Паника — разбор не прошёл.
pub fn raw(core: &Core) -> Vec<CoinNetworkInfo> {
    let at = At {
        venue: Some("bingx"),
        coin: None,
    };
    let own = own_nets::read(&core.inner.deps.settings, at).unwrap();
    let data = read("config_getall.json");
    let list = wallet::parse_networks(&data["data"], own, &v("bingx"));
    list.unwrap()
}

/// Адреса, которые BingX даёт только в сети ARBITRUM (в BEP20 их нет). Ошибок нет.
pub fn arb_only(raw: &[CoinNetworkInfo]) -> HashSet<Address> {
    let mut bsc = HashSet::new();
    let mut arb = HashSet::new();
    for i in raw {
        if let Some(a) = i.asset.contract {
            if i.network == "BEP20" {
                bsc.insert(a);
            } else if i.network == "ARBITRUM" {
                arb.insert(a);
            }
        }
    }
    arb.difference(&bsc).copied().collect()
}

/// Справочник сетей биржи для ядра (net_chain::prepare, как registry_sync.rs) — в Inner.networks; события SpotPairs из
/// записанного справочника пар — в состояние рынка. Возвращает: подготовленный список. Ошибок нет.
pub fn feed(core: &Core) -> Vec<CoinNetworkInfo> {
    let list = arb::core::net_chain::prepare(core, &v("bingx"), raw(core));
    let mut nets = core.inner.networks.write().unwrap();
    nets.insert(v("bingx"), Arc::new(list.clone()));
    drop(nets);
    let s = symbols::Symbols::default();
    let at = DateTime::from_timestamp_millis(SYMBOLS_AT_MS).unwrap();
    let pairs = read("symbols.json");
    let map = symbols::parse_list(&pairs["data"], "USDT").unwrap();
    s.load(map, at);
    let mut assets = vec![];
    for i in &list {
        if i.asset.contract.is_some() {
            assets.push(i.asset.clone());
        }
    }
    let ev = Snapshot::build(&s, &assets).event(v("bingx"), at);
    core.inner.market.write().unwrap().apply(ev);
    list
}

/// Все монеты справочника ядра (рабочие и неподвижные). Ошибок нет.
pub fn coins(core: &Core) -> Vec<CoinEntry> {
    let book = core.coins();
    let mut out = vec![];
    for c in book.all() {
        out.push(c.clone());
    }
    out
}

/// Набор подписки площадки DEX по справочнику — то же условие, что core/subscribe.rs wanted (монеты сети площадки,
/// chain::tag_of_dex, и subscribe::dex_tradable). Паника — сеть площадки не прочитана.
pub fn dex_set(core: &Core, dex: &str) -> Vec<CoinEntry> {
    let tag = arb::core::chain::tag_of_dex(core, &v(dex)).unwrap();
    let kinds = core.inner.kinds.read().unwrap().clone();
    let market = core.inner.market.read().unwrap();
    let mut out = vec![];
    for c in coins(core) {
        let tradable = arb::core::subscribe::dex_tradable(&c, &kinds, &market);
        if c.asset.chain == tag && tradable {
            out.push(c);
        }
    }
    out
}

/// Сколько монет списка в сети `tag` (None — без сети) с адресом из `only`. Ошибок нет.
pub fn count(list: &[CoinEntry], tag: Option<&ChainId>, only: &HashSet<Address>) -> usize {
    let mut n = 0;
    for c in list {
        let hit = c.asset.contract.is_some_and(|a| only.contains(&a));
        if c.asset.chain.as_ref() == tag && hit {
            n += 1;
        }
    }
    n
}

/// Сколько монет списка с сетью (Asset.chain = Some). Ошибок нет.
pub fn chained(list: &[CoinEntry]) -> usize {
    let mut n = 0;
    for c in list {
        if c.asset.chain.is_some() {
            n += 1;
        }
    }
    n
}
