//! Файл: tests/core_cex_nets_431.rs
//! Что делает: решение 431 (шаг 3 двух сетей) — биржи по сетям на записанных ответах API и настоящей SQLite:
//! (а) разбор с одним именем сети (как раньше) — адрес только у BSC; со своими сетями (venue.bsc_network +
//!     venue.arbitrum_network, gateway/own_nets.rs) — адрес и у сети Arbitrum: BingX «ARBITRUM», KuCoin «arbitrum»,
//!     Gate «ARBEVM» (USDT Arbitrum 0xFd08…Cbb9, USDT BSC 0x55d3…7955);
//! (б) ядро (core/net_chain.rs): площадки DEX Arbitrum нет — адрес записи Arbitrum снимается (справочник BSC прежний);
//!     есть — записи Arbitrum получают сеть «arbitrum», BSC — без сети; USDT по сети (usdt_address_in) не склеивается;
//! (в) имя сети перевода у биржи для сети Arbitrum — venue.arbitrum_network (core/transfers.rs own_name).
//! Записи: tests/data/bingx_2026-10-24/config_getall.json, tests/data/cex_samples_2026_11_04/kucoin_currencies_v3.json,
//! tests/data/cex_samples_2026_11_04/gate_spot_currencies.json. Ядро — t198_common (SQLite во временной папке).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues (разборы), arb::gateway::own_nets, arb::core (net_chain, registry_build, transfers).
//! Моков нет.

mod t198_common;

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::{Address, address};
use serde_json::{Value, json};

use arb::core::registry_build::{Networks, usdt_address, usdt_address_in};
use arb::core::types::{ChainId, CoinNetworkInfo, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::own_nets::{self, OwnNets};
use arb::settings::read::At;
use arb::settings::scope::Scope;

const DIR: &str = "tests/data/cex_samples_2026_11_04";
const USDT_BSC: Address = address!("55d398326f99059fF775485246999027B3197955");
const USDT_ARB: Address = address!("Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9");

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// JSON из файла записи. Паника — файла нет.
fn json_file(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

/// Адрес записи монеты в сети. Паника — записи нет.
fn addr(list: &[CoinNetworkInfo], ticker: &str, net: &str) -> Option<Address> {
    list.iter()
        .find(|i| i.asset.ticker == ticker && i.network == net)
        .unwrap_or_else(|| panic!("{ticker}/{net} нет в записи"))
        .asset
        .contract
}

/// Справочник BingX по своим сетям. Паника — разбор не прошёл.
fn bingx(own: impl Into<OwnNets>) -> Vec<CoinNetworkInfo> {
    let b = json_file("tests/data/bingx_2026-10-24/config_getall.json");
    arb::venues::bingx::wallet::parse_networks(&b["data"], own, &v("bingx")).unwrap()
}

/// Свои сети из настроек площадки. Паника — venue.bsc_network не задана.
fn own_of(s: &arb::settings::store::Store, venue: &str) -> OwnNets {
    let at = At {
        venue: Some(venue),
        coin: None,
    };
    own_nets::read(s, at).unwrap()
}

/// Задаёт настройку. Паника — запись не прошла.
async fn set(s: &arb::settings::store::Store, name: &str, scope: Scope, val: &str) {
    s.set(name, scope, Some(json!(val)), "тест").await.unwrap();
}

#[test]
fn parse_one_name_bsc_only_as_before() {
    let l = bingx("BEP20");
    assert_eq!(addr(&l, "USDT", "BEP20"), Some(USDT_BSC));
    assert_eq!(
        addr(&l, "USDT", "ARBITRUM"),
        None,
        "без имени Arbitrum — как раньше"
    );
}

#[tokio::test]
async fn parse_own_networks_three_venues() {
    let core = t198_common::start("cex-nets-431-parse").await;
    let s = core.inner.deps.settings.clone();
    for (venue, bsc, arb) in [
        ("bingx", "BEP20", "ARBITRUM"),
        ("kucoin", "bsc", "arbitrum"),
        ("gate", "BSC", "ARBEVM"),
    ] {
        set(&s, "venue.bsc_network", Scope::Venue(venue.into()), bsc).await;
        set(
            &s,
            "venue.arbitrum_network",
            Scope::Venue(venue.into()),
            arb,
        )
        .await;
    }
    let own = own_of(&s, "bingx");
    assert!(own.has("ARBITRUM") && own.is_bsc("BEP20") && !own.is_bsc("ARBITRUM"));
    let l = bingx(&own);
    assert_eq!(addr(&l, "USDT", "BEP20"), Some(USDT_BSC));
    assert_eq!(addr(&l, "USDT", "ARBITRUM"), Some(USDT_ARB));
    assert!(
        l.iter().all(|i| i.asset.chain.is_none()),
        "сеть ставит только ядро"
    );

    let mut k = json_file(&format!("{DIR}/kucoin_currencies_v3.json"));
    let kd = k.as_object_mut().unwrap().remove("data").unwrap();
    let ku =
        arb::venues::kucoin::networks_parse::parse(&kd, own_of(&s, "kucoin"), &v("kucoin"), false)
            .unwrap()
            .list;
    assert_eq!(addr(&ku, "USDT", "bsc"), Some(USDT_BSC));
    assert_eq!(addr(&ku, "USDT", "arbitrum"), Some(USDT_ARB));

    let gate = arb::venues::gate::networks_parse::parse_networks(
        &json_file(&format!("{DIR}/gate_spot_currencies.json")),
        &Value::Array(vec![]),
        own_of(&s, "gate"),
        &v("gate"),
        false,
        &HashMap::new(),
        &|_| Some(2),
    )
    .unwrap()
    .list;
    assert_eq!(addr(&gate, "USDT", "BSC"), Some(USDT_BSC));
    assert_eq!(addr(&gate, "USDT", "ARBEVM"), Some(USDT_ARB));
}

#[tokio::test]
async fn core_tags_records_by_chain() {
    let core = t198_common::start("cex-nets-431-core").await;
    let s = core.inner.deps.settings.clone();
    {
        let mut k = core.inner.kinds.write().unwrap();
        k.push((v("bingx"), VenueKind::Cex));
        k.push((v("bsc_dex"), VenueKind::Dex));
    }
    set(
        &s,
        "venue.bsc_network",
        Scope::Venue("bingx".into()),
        "BEP20",
    )
    .await;
    set(
        &s,
        "venue.arbitrum_network",
        Scope::Venue("bingx".into()),
        "ARBITRUM",
    )
    .await;
    let quote = HashMap::from([(v("bingx"), "USDT".to_string())]);
    let raw = bingx(own_of(&s, "bingx"));

    // (б) площадки DEX Arbitrum нет — адрес записи Arbitrum снят, справочник как с одним именем BSC.
    let one = arb::core::net_chain::tag(&core, &v("bingx"), raw.clone());
    let key = |l: &[CoinNetworkInfo]| -> Vec<(String, String, Option<Address>, Option<ChainId>)> {
        l.iter()
            .map(|i| {
                let a = &i.asset;
                (
                    a.ticker.clone(),
                    i.network.clone(),
                    a.contract,
                    a.chain.clone(),
                )
            })
            .collect()
    };
    assert_eq!(key(&one), key(&bingx("BEP20")), "как с одним именем BSC");
    let mut nets: Networks = HashMap::new();
    nets.insert(v("bingx"), Arc::new(one));
    assert_eq!(usdt_address(&nets, &quote).unwrap(), USDT_BSC);

    // Площадка DEX сети arbitrum есть — записи Arbitrum с сетью, BSC — без сети.
    core.inner
        .kinds
        .write()
        .unwrap()
        .push((v("arb_dex"), VenueKind::Dex));
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("arb_dex".into()), "arbitrum").await;
    set(&s, "dex.default_chain", Scope::Global, "bsc").await;
    let arb = ChainId("arbitrum".into());
    let two = arb::core::net_chain::tag(&core, &v("bingx"), raw);
    let usdt_arb = two
        .iter()
        .find(|i| i.asset.ticker == "USDT" && i.network == "ARBITRUM")
        .unwrap();
    assert_eq!(usdt_arb.asset.chain, Some(arb.clone()));
    assert_eq!(usdt_arb.asset.contract, Some(USDT_ARB));
    assert!(
        two.iter()
            .filter(|i| i.network == "BEP20")
            .all(|i| i.asset.chain.is_none()),
        "BSC — без сети, как раньше"
    );
    nets.insert(v("bingx"), Arc::new(two));
    assert_eq!(
        usdt_address(&nets, &quote).unwrap(),
        USDT_BSC,
        "адреса сетей не склеиваются"
    );
    assert_eq!(
        usdt_address_in(&nets, &quote, Some(&arb)).unwrap(),
        USDT_ARB
    );

    // (в) имя сети перевода биржи для Arbitrum.
    assert_eq!(
        arb::core::transfers::own_name(&core, &v("bingx"), Some(&arb)).as_deref(),
        Some("ARBITRUM")
    );
    assert_eq!(
        arb::core::transfers::own_name(&core, &v("bingx"), None).as_deref(),
        Some("BEP20")
    );
}
