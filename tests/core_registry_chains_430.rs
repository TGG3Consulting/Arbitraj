//! Файл: tests/core_registry_chains_430.rs
//! Что делает: решение 430 (шаг 2 двух сетей) — справочник по сетям (core/registry_chains.rs) на записанном справочнике
//! сетей BingX (tests/data/bingx_2026-10-24/config_getall.json, разбор адаптера bingx wallet::parse_networks):
//! (1) одна площадка DEX — одна группа «сеть по умолчанию» с ней (как раньше); две сети — две группы;
//! (2) записи бирж без сети (сегодняшний разбор — только BEP20) — все в сети по умолчанию, USDT — адрес BEP20, у
//!     Arbitrum адреса нет (биржи отдадут адреса по сетям — шаг 3);
//! (3) записи той же биржи по сети ARBITRUM с сетью «arbitrum» — USDT Arbitrum из ответа BingX, USDT BSC прежний;
//! (4) виды площадок сети (chain::kinds_in): биржи + DEX только этой сети.
//! Ядро — t198_common (SQLite во временной папке; bingx, mexc — биржи, bsc_dex — DEX).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{chain, registry_build, registry_chains}, arb::venues::bingx::wallet. Моков нет.

mod t198_common;

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::{Address, address};
use serde_json::{Value, json};

use arb::core::chain;
use arb::core::registry_build::{Networks, usdt_address};
use arb::core::registry_chains::{self, Group};
use arb::core::types::{ChainId, CoinNetworkInfo, VenueId};
use arb::gateway::contract::VenueKind;
use arb::settings::scope::Scope;
use arb::venues::bingx::wallet;

/// USDT в сети BSC (BingX networkList, network BEP20).
const USDT_BSC: Address = address!("0x55d398326f99059ff775485246999027b3197955");
/// USDT в сети Arbitrum (BingX networkList, network ARBITRUM).
const USDT_ARB: Address = address!("0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9");

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Справочник сетей BingX по сети `net` из записи. Паника — файла нет или разбор не прошёл.
fn bingx(net: &str) -> Vec<CoinNetworkInfo> {
    let raw = std::fs::read_to_string("tests/data/bingx_2026-10-24/config_getall.json").unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    wallet::parse_networks(&data["data"], net, &v("bingx")).unwrap()
}

/// Задаёт настройку. Паника — запись не прошла.
async fn set(s: &arb::settings::store::Store, name: &str, scope: Scope, val: &str) {
    s.set(name, scope, Some(json!(val)), "тест").await.unwrap();
}

#[tokio::test]
async fn registry_by_chain() {
    let core = t198_common::start("registry-chains-430").await;
    let s = core.inner.deps.settings.clone();
    let one = registry_chains::groups(&core, &chain::dexes(&core));
    assert_eq!(
        one,
        vec![Group {
            tag: None,
            dex: vec![v("bsc_dex")]
        }],
        "одна DEX — как раньше, без настроек"
    );

    let bsc = bingx("BEP20");
    assert!(
        bsc.iter().all(|i| i.asset.chain.is_none()),
        "биржа сети не ставит (шаг 3)"
    );
    let mut nets: Networks = HashMap::new();
    nets.insert(v("bingx"), Arc::new(bsc.clone()));
    let quote = HashMap::from([(v("bingx"), "USDT".to_string())]);
    assert_eq!(
        registry_chains::nets_of(&nets, None)[&v("bingx")].len(),
        bsc.len()
    );
    assert_eq!(usdt_address(&nets, &quote).unwrap(), USDT_BSC);

    core.inner
        .kinds
        .write()
        .unwrap()
        .push((v("arb_dex"), VenueKind::Dex));
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("arb_dex".into()), "arbitrum").await;
    set(&s, "dex.default_chain", Scope::Global, "bsc").await;
    let arb = ChainId("arbitrum".into());
    let two = registry_chains::groups(&core, &chain::dexes(&core));
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].dex, vec![v("bsc_dex")]);
    assert_eq!(two[1].tag, Some(arb.clone()));
    assert!(
        registry_chains::usdt_other(&two, &nets, &quote).is_empty(),
        "адресов Arbitrum от бирж ещё нет"
    );

    // Шаг 3: биржа отдаст записи по сети ARBITRUM — с сетью «arbitrum».
    let mut on_arb = bingx("ARBITRUM");
    for i in &mut on_arb {
        i.asset.chain = Some(arb.clone());
    }
    let mut both = bsc;
    both.extend(on_arb);
    nets.insert(v("bingx"), Arc::new(both));
    let other = registry_chains::usdt_other(&two, &nets, &quote);
    assert_eq!(other.get(&arb), Some(&USDT_ARB));
    assert_eq!(
        usdt_address(&registry_chains::nets_of(&nets, None), &quote).unwrap(),
        USDT_BSC
    );

    let k = chain::kinds_in(&core, Some(&arb));
    assert!(k.iter().any(|(x, _)| *x == v("arb_dex")));
    assert!(!k.iter().any(|(x, _)| *x == v("bsc_dex")));
    let k = chain::kinds_in(&core, None);
    assert!(k.iter().any(|(x, _)| *x == v("bsc_dex")));
    assert!(!k.iter().any(|(x, _)| *x == v("arb_dex")));
}
