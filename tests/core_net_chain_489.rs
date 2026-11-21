//! Файл: tests/core_net_chain_489.rs
//! Что делает: решение 489 — площадка DEX участвует в сетях ядра только при venue.enabled = да, на записанном справочнике
//! сетей BingX (tests/data/bingx_2026-10-24/config_getall.json: у монет есть сеть ARBITRUM с contractAddress) и
//! справочнике пар (symbols.json), на работающем ядре с настоящей SQLite (t489_common):
//! (1) инцидент 21.11.2026: bsc_dex включена, arb_dex зарегистрирована, но НЕ включена — net_chain снимает адрес у
//!     записей ARBITRUM; в справочнике нет монеты без сети с арбитрумным адресом и нет монет сети arbitrum; набор
//!     подписки bsc_dex их не содержит; группа справочника одна (bsc_dex);
//! (2) arb_dex включена — записи ARBITRUM с сетью «arbitrum» и адресом, монеты уходят в сеть arbitrum, в набор bsc_dex не
//!     попадают;
//! (3) справочник сетей ещё с привязкой «arb_dex включена», а её уже выключили (окно до перечитывания) — пересборка не
//!     пускает записи с сетью в сеть по умолчанию (rebuild_build.rs);
//! (4) tag_of_dex выключенной площадки — её сеть по реестру (решение 196: её сделки и учёт), не сеть по умолчанию;
//!     kinds_in(None) её не содержит; active_dexes / in_play — без неё; net_chain::affects — venue.enabled DEX.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (chain, net_chain, registry_chains, rebuild, subscribe), t489_common. Моков нет.

mod t489_common;

use serde_json::json;

use arb::core::chain;
use arb::core::registry_chains::{self, Group};
use arb::settings::live::Change;
use arb::settings::scope::Scope;
use t489_common::{arb_chain, arb_only, chained, coins, count, dex_set, feed, raw, start, v};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incident_disabled_arb_dex_strips_arbitrum() {
    let core = start("net-chain-489-incident", true, false).await;
    let only = arb_only(&raw(&core));
    assert!(!only.is_empty(), "в записи есть адреса только ARBITRUM");
    let list = feed(&core);
    for i in list.iter().filter(|i| i.network == "ARBITRUM") {
        assert_eq!(i.asset.contract, None, "адрес ARBITRUM снят");
        assert_eq!(i.asset.chain, None);
    }
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
    assert_eq!(count(&all, None, &only), 0, "без сети нет ARBITRUM");
    assert_eq!(chained(&all), 0, "монет сети arbitrum нет");
    let sub = dex_set(&core, "bsc_dex");
    assert_eq!(count(&sub, None, &only), 0, "подписка bsc_dex чистая");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabled_arb_dex_gets_its_network() {
    let core = start("net-chain-489-enabled", true, true).await;
    let only = arb_only(&raw(&core));
    let list = feed(&core);
    let usdt = list
        .iter()
        .find(|i| i.asset.ticker == "USDT" && i.network == "ARBITRUM")
        .unwrap();
    assert_eq!(usdt.asset.chain, Some(arb_chain()));
    assert!(usdt.asset.contract.is_some());
    for i in list.iter().filter(|i| i.network == "BEP20") {
        assert_eq!(i.asset.chain, None, "BSC — без сети");
    }
    let groups = registry_chains::groups(&core, &chain::dexes(&core));
    assert_eq!(groups.len(), 2);
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    let a = arb_chain();
    assert!(count(&all, Some(&a), &only) > 0, "монеты arbitrum есть");
    assert_eq!(count(&all, None, &only), 0, "без сети нет ARBITRUM");
    let sub = dex_set(&core, "bsc_dex");
    assert_eq!(chained(&sub), 0, "bsc_dex без монет arbitrum");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_tagged_networks_stay_out_of_default_chain() {
    let core = start("net-chain-489-stale", true, true).await;
    let only = arb_only(&raw(&core));
    let list = feed(&core);
    let a = arb_chain();
    assert!(list.iter().any(|i| i.asset.chain.as_ref() == Some(&a)));
    // Выключили arb_dex; справочник сетей бирж ещё прежний (с сетью arbitrum у записей ARBITRUM).
    let scope = Scope::Venue("arb_dex".into());
    t489_common::set(&core, "venue.enabled", scope, json!(false)).await;
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    assert_eq!(count(&all, None, &only), 0, "записи с сетью — не без сети");
    assert_eq!(chained(&all), 0, "группы arbitrum нет");
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
