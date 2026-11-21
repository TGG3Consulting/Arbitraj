//! Файл: tests/core_net_chain_489.rs
//! Что делает: решение 489 — площадка DEX участвует в сетях ядра только «в игре» (venue.enabled = да или её держат
//! идущие сделки), на записанных ответах BingX 2026-11-21 (tests/data/bingx_2026-11-21: config_getall.json — 50 монет с
//! адресом только в сети ARBITRUM, symbols.json — их пары) на работающем ядре с настоящей SQLite (t489_common):
//! (1) инцидент 21.11.2026: bsc_dex включена, arb_dex зарегистрирована, не включена, сделок нет — адрес записей
//!     ARBITRUM снят; монет без сети с адресом «только ARBITRUM» 0, монет сети arbitrum 0, в наборе подписки bsc_dex их
//!     нет. На коде базы (a3f8f568) та же проверка падала: net_chain.rs:37 брал все зарегистрированные DEX → записи
//!     ARBITRUM с сетью и адресом (net_chain.rs:71); сборка из включённых давала одну группу → rebuild_build.rs:61,70
//!     (single) отдавал справочник сетей без nets_of → registry_build.rs:166 ставил монете chain: None с адресом первой
//!     записи с адресом (presence.rs:38) — VELA, MAGIC … без сети; MAGIC с торгуемой парой — в набор bsc_dex
//!     (subscribe.rs dex_tradable);
//! (2) arb_dex включена — монеты «только ARBITRUM» в сети arbitrum с адресом, без сети их нет, набор bsc_dex без них,
//!     набор arb_dex — с ними;
//! (3) окно до перечитывания справочника сетей: arb_dex выключили, справочник ещё с сетью arbitrum — записи с сетью в
//!     сеть по умолчанию не идут (rebuild_build.rs), монет arbitrum нет;
//! (4) предикаты: in_play, active_dexes, tag_of_dex выключенной — сеть по реестру, kinds_in, net_chain::affects.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (chain, net_chain, registry_chains, rebuild, subscribe), t489_common. Моков нет.

mod t489_common;

use alloy_primitives::address;
use serde_json::json;

use arb::core::chain;
use arb::core::registry_chains::{self, Group};
use arb::settings::live::Change;
use arb::settings::scope::Scope;
use t489_common::book::{coins, dex_set, pick};
use t489_common::{NEW, arb_chain, arb_only, feed, raw, start, v};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incident_disabled_arb_dex_strips_arbitrum() {
    let core = start("net-chain-489-incident", true, false).await;
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    // Факт записи: USDT (котируемая) + 49 монет; VELA — единственная сеть ARBITRUM.
    assert_eq!(only.len(), 50);
    let vela = address!("088cd8f5ef3652623c22d48b1605dcfe860cd704");
    assert!(only.contains(&vela));
    let list = feed(&core, NEW, raw);
    for i in list.iter().filter(|i| i.network == "ARBITRUM") {
        assert_eq!(i.asset.contract, None, "адрес ARBITRUM снят");
        assert_eq!(i.asset.chain, None);
    }
    // Замок рынка — только на время этого выражения: ниже rebuild(...).await (clippy await_holding_lock).
    let magic = core.inner.market.read().unwrap().spot_pair(&v("bingx"), "MAGIC");
    assert!(magic, "MAGIC торгуется — на базе ушла бы в bsc_dex");
    assert_eq!(chain::active_dexes(&core), vec![v("bsc_dex")]);
    let groups = registry_chains::groups(&core, &chain::dexes(&core));
    let one = Group {
        tag: None,
        dex: vec![v("bsc_dex")],
    };
    assert_eq!(groups, vec![one]);
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    assert!(!all.is_empty(), "справочник собран");
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    let chained = all.iter().filter(|c| c.asset.chain.is_some()).count();
    assert_eq!(chained, 0, "сети arbitrum нет");
    let sub = dex_set(&core, "bsc_dex");
    assert!(!sub.is_empty(), "набор bsc_dex есть");
    let leak = pick(&sub, None, &only);
    assert!(leak.is_empty(), "подписка bsc_dex чистая");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabled_arb_dex_gets_its_network() {
    let core = start("net-chain-489-enabled", true, true).await;
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    let list = feed(&core, NEW, raw);
    let vela = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(vela.asset.chain, Some(arb_chain()));
    assert!(vela.asset.contract.is_some());
    for i in list.iter().filter(|i| i.network == "BEP20") {
        assert_eq!(i.asset.chain, None, "BSC — без сети");
    }
    let groups = registry_chains::groups(&core, &chain::dexes(&core));
    assert_eq!(groups.len(), 2);
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    let a = arb_chain();
    let on_arb = pick(&all, Some(&a), &only);
    assert!(on_arb.len() > 40, "монеты arbitrum есть");
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    let sub = dex_set(&core, "bsc_dex");
    let chained = sub.iter().filter(|c| c.asset.chain.is_some()).count();
    assert_eq!(chained, 0, "bsc_dex без arbitrum");
    let arb_sub = dex_set(&core, "arb_dex");
    let magic = arb_sub.iter().any(|c| c.asset.ticker == "MAGIC");
    assert!(magic, "MAGIC в arb_dex");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_tagged_networks_stay_out_of_default_chain() {
    let core = start("net-chain-489-stale", true, true).await;
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    let list = feed(&core, NEW, raw);
    let a = arb_chain();
    assert!(list.iter().any(|i| i.asset.chain.as_ref() == Some(&a)));
    // Выключили arb_dex; справочник сетей бирж ещё прежний (с сетью arbitrum у записей ARBITRUM).
    let scope = Scope::Venue("arb_dex".into());
    t489_common::set(&core, "venue.enabled", scope, json!(false)).await;
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    let leak = pick(&all, None, &only);
    assert!(leak.is_empty(), "записи с сетью — не без сети");
    let chained = all.iter().filter(|c| c.asset.chain.is_some()).count();
    assert_eq!(chained, 0, "группы arbitrum нет");
}

/// Изменение настройки для net_chain::affects. Ошибок нет.
fn change(name: &str, scope: Scope) -> Change {
    Change {
        name: name.into(),
        scope,
        old: None,
        new: Some(json!(true)),
        by: "тест".into(),
        at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn disabled_dex_predicates() {
    let core = start("net-chain-489-pred", true, false).await;
    assert!(chain::in_play(&core, &v("bsc_dex")));
    assert!(!chain::in_play(&core, &v("arb_dex")));
    assert!(!chain::in_play(&core, &v("bingx")), "биржа — не DEX");
    assert_eq!(chain::dexes(&core), vec![v("bsc_dex"), v("arb_dex")]);
    // Сеть выключенной площадки — по реестру (её сделки и учёт), не сеть по умолчанию.
    let tag = chain::tag_of_dex(&core, &v("arb_dex"));
    assert_eq!(tag, Ok(Some(arb_chain())));
    assert_eq!(chain::tag_of_dex(&core, &v("bsc_dex")), Ok(None));
    let k = chain::kinds_in(&core, None);
    assert!(!k.iter().any(|(x, _)| *x == v("arb_dex")));
    assert!(k.iter().any(|(x, _)| *x == v("bsc_dex")));
    let affects = arb::core::net_chain::affects;
    let dex = || Scope::Venue("arb_dex".into());
    assert!(affects(&core, &change("venue.enabled", dex())));
    assert!(affects(&core, &change("dex.chain", dex())));
    assert!(affects(&core, &change(chain::DEFAULT, Scope::Global)));
    assert!(!affects(&core, &change("venue.quote_asset", dex())));
    let cex = Scope::Venue("bingx".into());
    assert!(!affects(&core, &change("venue.enabled", cex)));
}
