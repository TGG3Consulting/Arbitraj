//! Файл: tests/t489_common/mod.rs
//! Что делает: решение 489 — общие помощники тестов «DEX в игре»: ядро t198_common (SQLite во временной папке) с биржей
//! bingx (venue.bsc_network = BEP20, venue.arbitrum_network = ARBITRUM — как в инциденте 21.11.2026) и площадками DEX
//! bsc_dex (включена, dex.chain = bsc) и arb_dex (dex.chain = arbitrum; включена — по тесту), dex.default_chain = bsc;
//! справочник сетей BingX из записанного ответа (папка записи — параметр: bingx_2026-10-24 или bingx_2026-11-21,
//! config_getall.json, разбор адаптера по своим сетям); справочник пар BingX из записанного symbols.json — событие
//! SpotPairs, как его шлёт адаптер; адреса, которые BingX даёт только в сети ARBITRUM; идущая сделка через arb_dex
//! (решение 196: держит площадку); набор подписки DEX по тому же условию, что core/subscribe.rs wanted.
//! Кто вызывает: tests/core_net_chain_489.rs, tests/core_net_chain_489_bsc.rs, tests/core_net_chain_489_held.rs.
//! От чего зависит: arb::core (chain, deal, feed_hold, net_chain, subscribe), arb::venues::bingx, arb::gateway::own_nets,
//! t198_common. Моков нет.

#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::Arc;

use alloy_primitives::Address;
use chrono::DateTime;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::types::{Asset, ChainId, CoinNetworkInfo, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::own_nets;
use arb::settings::read::At;
use arb::settings::scope::Scope;
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::{symbols, wallet};

#[path = "../t198_common/mod.rs"]
mod t198_common;

pub mod book;

/// Запись 2026-10-24: в сети ARBITRUM у BingX только USDT (котируемая монета, в справочник монет не входит).
pub const OLD: &str = "bingx_2026-10-24";
/// Запись 2026-11-21 (сырой журнал бота на сервере, отбор — README папки): 50 монет с адресом только в ARBITRUM.
pub const NEW: &str = "bingx_2026-11-21";

/// Время ответа symbols записи (поле `t` сырого журнала, мс; README папки записи). Паника — время записи не внесено.
pub fn symbols_at_ms(dir: &str) -> i64 {
    match dir {
        // tests/data/bingx_2026-10-24/README.md.
        OLD => 1792868538956,
        // tests/data/bingx_2026-11-21/README.md.
        NEW => 1795259800908,
        _ => panic!("время symbols записи {dir} не внесено"),
    }
}

/// Площадка по id. Ошибок нет.
pub fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Сеть Arbitrum ядра (dex.chain площадки arb_dex). Ошибок нет.
pub fn arb_chain() -> ChainId {
    ChainId("arbitrum".into())
}

/// JSON записи BingX из папки `dir`. Паника — файла нет.
fn read(dir: &str, name: &str) -> Value {
    let path = format!("tests/data/{dir}/{name}");
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
        // Записи — прошлые: справочник пар должен считаться свежим и сейчас (как tests/core_cold_coins.rs).
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

/// Справочник сетей BingX записи `dir` по своим сетям из настроек (как адаптер). Паника — разбор не прошёл.
pub fn raw(core: &Core, dir: &str) -> Vec<CoinNetworkInfo> {
    let at = At {
        venue: Some("bingx"),
        coin: None,
    };
    let own = own_nets::read(&core.inner.deps.settings, at).unwrap();
    let data = read(dir, "config_getall.json");
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

/// Справочник сетей `raw` для ядра (net_chain::prepare, как registry_sync.rs) — в Inner.networks; событие SpotPairs из
/// записанного справочника пар папки `dir` по монетам разбора с адресом (одно и то же при любой привязке к сети — пары
/// биржа знает по тикеру) — в состояние рынка. Возвращает: подготовленный список. Ошибок нет.
pub fn feed(core: &Core, dir: &str, raw: Vec<CoinNetworkInfo>) -> Vec<CoinNetworkInfo> {
    let mut assets = vec![];
    for i in &raw {
        if i.asset.contract.is_some() {
            assets.push(i.asset.clone());
        }
    }
    let list = arb::core::net_chain::prepare(core, &v("bingx"), raw);
    let mut nets = core.inner.networks.write().unwrap();
    nets.insert(v("bingx"), Arc::new(list.clone()));
    drop(nets);
    let s = symbols::Symbols::default();
    let pairs = read(dir, "symbols.json");
    let at = DateTime::from_timestamp_millis(symbols_at_ms(dir)).unwrap();
    let map = symbols::parse_list(&pairs["data"], "USDT").unwrap();
    s.load(map, at);
    let ev = Snapshot::build(&s, &assets).event(v("bingx"), at);
    core.inner.market.write().unwrap().apply(ev);
    list
}

/// Идущая сделка bingx → `dex` монеты сети `chain` (шаг «покупка» — открыта): по решению 196 держит площадку `dex`.
/// Ошибок нет.
pub fn hold(core: &Core, id: &str, dex: &str, chain: Option<ChainId>) {
    let dir = Direction {
        asset: Asset {
            chain,
            ticker: "USDT".into(),
            contract: None,
        },
        buy_on: v("bingx"),
        sell_on: v(dex),
    };
    let d = Deal::new(id.into(), dir, json!({}), Decimal::ZERO);
    core.inner.deals.lock().unwrap().insert(id.into(), d);
}

/// Сделка `id` закрыта (шаг Closed) и сохранена — как ledger.rs: запись в учёт памяти, затем feed_hold::on_saved.
/// Паника — сделки нет.
pub fn close(core: &Core, id: &str) {
    let mut deals = core.inner.deals.lock().unwrap();
    let d = deals.get_mut(id).unwrap();
    d.step = Step::Closed;
    let rec = d.clone();
    drop(deals);
    arb::core::feed_hold::on_saved(core, &rec);
}
