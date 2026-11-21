//! Файл: tests/core_net_chain_489_mexc.rs
//! Что делает: решение 489 — MEXC по своим сетям на записанном ответе config/getall 2026-11-21
//! (tests/data/mexc_2026-11-21/config_getall.json, отбор — README папки): код сети MEXC — netWork («BSC», «ARB»;
//! разбор сравнивает настройку с ним — venues/mexc/networks_parse.rs:73,80). Свои сети {BSC} — адрес только у BSC (как
//! раньше); {BSC, ARB} (будущая venue.arbitrum_network[mexc] = «ARB») — адрес и у ARB. Ядро (net_chain): arb_dex не
//! включена и сделок нет — адрес ARB снят; включена — ARB с сетью arbitrum; один адрес в BSC и ARB (MLK) — две записи
//! разных сетей.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::mexc (networks_parse, symbols), arb::core::net_chain, arb::gateway::own_nets,
//! t489_common. Моков нет.

mod t489_common;

use serde_json::{Value, json};

use arb::core::Core;
use arb::core::types::CoinNetworkInfo;
use arb::gateway::contract::VenueKind;
use arb::gateway::own_nets;
use arb::settings::read::At;
use arb::settings::scope::Scope;
use arb::venues::mexc::{networks_parse, symbols};
use t489_common::{arb_chain, set, start, v};

/// Справочник сетей MEXC записи по своим сетям из настроек. Паника — файла нет или разбор не прошёл.
fn mexc(core: &Core) -> Vec<CoinNetworkInfo> {
    let path = "tests/data/mexc_2026-11-21/config_getall.json";
    let raw = std::fs::read_to_string(path).unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    let at = At {
        venue: Some("mexc"),
        coin: None,
    };
    let own = own_nets::read(&core.inner.deps.settings, at).unwrap();
    let s = symbols::Symbols::default();
    let p = networks_parse::parse_networks(&data, own, &v("mexc"), false, &s);
    p.unwrap().list
}

/// Запись монеты в сети. Паника — записи нет.
fn rec<'a>(l: &'a [CoinNetworkInfo], ticker: &str, net: &str) -> &'a CoinNetworkInfo {
    let i = l.iter().find(|i| i.asset.ticker == ticker && i.network == net);
    i.unwrap()
}

#[tokio::test]
async fn mexc_arb_network_by_settings() {
    let core = start("net-chain-489-mexc", true, false).await;
    core.inner
        .kinds
        .write()
        .unwrap()
        .push((v("mexc"), VenueKind::Cex));
    let mx = Scope::Venue("mexc".into());
    set(&core, "venue.bsc_network", mx.clone(), json!("BSC")).await;
    let one = mexc(&core);
    assert!(rec(&one, "RDNT", "BSC").asset.contract.is_some());
    let r = rec(&one, "RDNT", "ARB");
    assert_eq!(r.asset.contract, None, "без ARB — как раньше");
    set(&core, "venue.arbitrum_network", mx, json!("ARB")).await;
    let two = mexc(&core);
    assert!(rec(&two, "RDNT", "ARB").asset.contract.is_some());
    assert!(rec(&two, "BFR", "ARB").asset.contract.is_some());
    let mlk_bsc = rec(&two, "MLK", "BSC").asset.contract;
    let mlk_arb = rec(&two, "MLK", "ARB").asset.contract;
    assert!(mlk_bsc.is_some());
    assert_eq!(mlk_bsc, mlk_arb, "один адрес");

    // arb_dex не включена, сделок нет — сети arbitrum нет: адрес ARB снят.
    let off = arb::core::net_chain::prepare(&core, &v("mexc"), two.clone());
    assert_eq!(rec(&off, "RDNT", "ARB").asset.contract, None);
    assert!(off.iter().all(|i| i.asset.chain.is_none()));
    // arb_dex включена — записи ARB в сети arbitrum, BSC — без сети.
    let a = Scope::Venue("arb_dex".into());
    set(&core, "venue.enabled", a, json!(true)).await;
    let on = arb::core::net_chain::prepare(&core, &v("mexc"), two);
    assert_eq!(rec(&on, "MLK", "ARB").asset.chain, Some(arb_chain()));
    assert_eq!(rec(&on, "MLK", "BSC").asset.chain, None, "BSC без сети");
    assert!(rec(&on, "BFR", "ARB").asset.contract.is_some());
}
