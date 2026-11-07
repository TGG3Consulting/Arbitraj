//! Файл: tests/core_coin_by_address.rs
//! Что делает: решение 211 (монета = адрес контракта BSC) на записанных справочниках сетей трёх бирж: Gate
//! (cex_samples_2026_11_04/gate_spot_currencies.json), KuCoin (kucoin_currencies_v3.json) и BingX
//! (bingx_2026-10-24/config_getall.json). Проверяется сборка справочника ядра (core/registry_build.rs) и переводчик
//! тикеров (core/ticker_index.rs index_of + venue_ticker.rs):
//! - один адрес под разными тикерами у разных бирж — ОДНА монета, у каждой биржи свой тикер (Presence::ticker);
//! - один тикер с разными адресами у разных бирж — РАЗНЫЕ монеты;
//! - монета для вызова биржи (on_venue) несёт тикер этой биржи и тот же адрес; обратная карта (биржа, её тикер) →
//!   монета ядра;
//! - равенство монеты — по адресу (тикер не участвует).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{gate, kucoin, bingx} (разбор сетей), arb::core::{registry, ticker_index}. Моков нет.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::market_state::MarketState;
use arb::core::registry::{self, Inputs};
use arb::core::ticker_index::index_of;
use arb::core::types::{Asset, CoinNetworkInfo, Pool, VenueId};
use arb::gateway::contract::VenueKind;

const DIR: &str = "tests/data/cex_samples_2026_11_04";

fn json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Сети трёх бирж из записанных ответов.
fn networks() -> HashMap<VenueId, Vec<CoinNetworkInfo>> {
    let gate = arb::venues::gate::networks_parse::parse_networks(
        &json(&format!("{DIR}/gate_spot_currencies.json")),
        &Value::Array(vec![]),
        "BSC",
        &v("gate"),
        false,
        &HashMap::new(),
        &|_| Some(2),
    )
    .unwrap()
    .list;
    let mut k = json(&format!("{DIR}/kucoin_currencies_v3.json"));
    let kd = k.as_object_mut().unwrap().remove("data").unwrap();
    let kucoin = arb::venues::kucoin::networks_parse::parse(&kd, "bsc", &v("kucoin"), false)
        .unwrap()
        .list;
    let b = json("tests/data/bingx_2026-10-24/config_getall.json");
    let bingx =
        arb::venues::bingx::wallet::parse_networks(&b["data"], "BEP20", &v("bingx")).unwrap();
    HashMap::from([
        (v("gate"), gate),
        (v("kucoin"), kucoin),
        (v("bingx"), bingx),
    ])
}

/// Адрес → (биржа → тикер) по сетям BSC с адресом.
fn by_address(
    n: &HashMap<VenueId, Vec<CoinNetworkInfo>>,
) -> BTreeMap<Address, BTreeMap<String, BTreeSet<String>>> {
    let mut out: BTreeMap<Address, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for (venue, list) in n {
        for i in list.iter().filter(|i| i.asset.ticker != "USDT") {
            if let Some(a) = i.asset.bsc_contract {
                out.entry(a)
                    .or_default()
                    .entry(venue.0.clone())
                    .or_default()
                    .insert(i.asset.ticker.clone());
            }
        }
    }
    out
}

#[test]
fn one_coin_per_address_venue_tickers_kept() {
    let n = networks();
    let kinds = vec![
        (v("bingx"), VenueKind::Cex),
        (v("gate"), VenueKind::Cex),
        (v("kucoin"), VenueKind::Cex),
    ];
    let networks = n
        .iter()
        .map(|(k, l)| (k.clone(), std::sync::Arc::new(l.clone())))
        .collect();
    let quote = kinds
        .iter()
        .map(|(k, _)| (k.clone(), "USDT".to_string()))
        .collect();
    let bsc = HashMap::from([
        (v("bingx"), "BEP20".to_string()),
        (v("gate"), "BSC".to_string()),
        (v("kucoin"), "bsc".to_string()),
    ]);
    let fresh = kinds.iter().map(|(k, _)| (k.clone(), Ok(()))).collect();
    let market = MarketState::default();
    let pool_ok = |_: &Pool| Ok(());
    let inter = HashMap::new();
    let coins = registry::build(&Inputs {
        venues: &kinds,
        networks: &networks,
        usdt: None,
        quote: &quote,
        bsc_network: &bsc,
        pairs_fresh: &fresh,
        market: &market,
        pool_ok: &pool_ok,
        intermediate: &inter,
    });
    let addr = by_address(&n);
    // Одна запись справочника на адрес.
    let mut seen: HashMap<Address, usize> = HashMap::new();
    for c in coins.iter().filter(|c| c.asset.bsc_contract.is_some()) {
        *seen.entry(c.asset.bsc_contract.unwrap()).or_default() += 1;
    }
    assert!(seen.values().all(|&k| k == 1), "у адреса несколько записей");
    assert_eq!(seen.len(), addr.len(), "каждый адрес — в справочнике");
    let idx = index_of(&coins);
    // Адреса с разными тикерами у разных бирж — одна монета; on_venue даёт тикер биржи.
    let mut cross = 0;
    for (a, venues) in &addr {
        let single: Vec<(&String, &String)> = venues
            .iter()
            .filter(|(_, t)| t.len() == 1)
            .map(|(v, t)| (v, t.iter().next().unwrap()))
            .collect();
        let names: BTreeSet<&String> = single.iter().map(|(_, t)| *t).collect();
        if single.len() < 2 || names.len() < 2 {
            continue;
        }
        cross += 1;
        let c = coins
            .iter()
            .find(|c| c.asset.bsc_contract == Some(*a))
            .unwrap();
        for (venue, t) in &single {
            let p = c.presence.iter().find(|p| &p.venue.0 == *venue).unwrap();
            assert_eq!(&&p.ticker, t);
            let on = idx.on_venue(&c.asset, &v(venue));
            assert_eq!((&on.ticker, on.bsc_contract), (*t, Some(*a)));
            assert_eq!(idx.get(&v(venue), t).as_ref(), Some(&c.asset));
        }
    }
    // Один тикер с разными адресами у разных бирж — разные монеты (по адресу).
    let mut by_ticker: HashMap<String, BTreeSet<Address>> = HashMap::new();
    for (a, venues) in &addr {
        for t in venues.values().flatten() {
            by_ticker.entry(t.clone()).or_default().insert(*a);
        }
    }
    let twins = by_ticker.values().filter(|s| s.len() > 1).count();
    for s in by_ticker.values().filter(|s| s.len() > 1) {
        for a in s {
            assert_eq!(seen.get(a), Some(&1));
        }
    }
    eprintln!(
        "адресов {}, под разными тикерами {cross}, тикеров с разными адресами {twins}",
        addr.len()
    );
    assert!(
        cross > 0,
        "в записях нет адреса под разными тикерами — тест ничего не проверил"
    );
}

#[test]
fn asset_equality_is_by_address() {
    let a: Address = "0x03aa6298f1370642642415edc0db8b957783e8d6"
        .parse()
        .unwrap();
    let nmt = Asset {
        ticker: "NMT".into(),
        bsc_contract: Some(a),
    };
    let netmind = Asset {
        ticker: "NETMIND".into(),
        bsc_contract: Some(a),
    };
    assert_eq!(nmt, netmind);
    let set: std::collections::HashSet<Asset> = [nmt.clone(), netmind].into_iter().collect();
    assert_eq!(set.len(), 1);
    let gate_nmt = Asset {
        ticker: "NMT".into(),
        bsc_contract: None,
    };
    assert_ne!(nmt, gate_nmt, "адрес только у одной — разные монеты");
    assert_eq!(
        gate_nmt,
        Asset {
            ticker: "NMT".into(),
            bsc_contract: None
        }
    );
}
