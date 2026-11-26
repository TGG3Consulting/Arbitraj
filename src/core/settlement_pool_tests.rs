//! Файл: core/settlement_pool_tests.rs.
//! Что делает: повторяет холодный справочник с реальными ответами Gate для Base и Ethereum.
//! Кто вызывает: GitHub Actions.
//! От чего зависит: адреса ERC-20 в строках Gate, состав пары Base из записанного пула Sushi.
//! Настройки: локальная временная БД с двумя независимыми площадками.
use super::*;
use crate::{
    core::{engine::wr, types::ChainId},
    settings::scope::Scope,
    venues::gate::networks_parse,
};
use serde_json::{Value, json};
use std::sync::Arc;

#[path = "settlement_pool_test_setup.rs"]
mod setup;

#[tokio::test]
async fn base_usdc_pool_and_ethereum_usdt_use_independent_providers_at_cold_start() {
    let root = std::env::temp_dir().join(format!(
        "arb-cold-settlement-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let core = crate::core::test_core::open(&root).await;
    let s = &core.inner.deps.settings;
    let base = VenueId("base_dex".into());
    let eth = VenueId("eth_dex".into());
    let cex = VenueId("gate".into());
    let stable: Value =
        serde_json::from_str(include_str!("../../tests/data/base_usdc_gate_market.json")).unwrap();
    let providers: Value = serde_json::from_str(include_str!(
        "../../tests/data/gate_settlement_networks.json"
    ))
    .unwrap();
    let pool: Value = serde_json::from_str(include_str!(
        "../../tests/data/first_run_dex_2026-11-26/verified_v3_base.json"
    ))
    .unwrap();
    let gate_base = &stable["network"]["chains"][0];
    let gate_eth = &providers["ethereum"]["ethereum"][0];
    let usdc: Address = gate_base["addr"].as_str().unwrap().parse().unwrap();
    let usdt: Address = gate_eth["addr"].as_str().unwrap().parse().unwrap();
    assert!(
        providers["base"]["matching_rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        pool["sushiswap-v3-base"]["pool"]["tokens"][1]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap(),
        usdc
    );
    let scope = |id: &VenueId| Scope::Venue(id.0.clone());
    setup::install(
        &core,
        &base,
        &eth,
        &cex,
        (gate_base, gate_eth, &providers, &pool),
    )
    .await;
    let parsed = |body: &Value, name: &str, chain: &str| {
        let mut rows = networks_parse::parse_networks(
            body,
            &json!([]),
            name,
            &cex,
            true,
            &Default::default(),
            &|_| None,
        )
        .unwrap()
        .list;
        let mut row = rows.drain(..).find(|r| r.network == name).unwrap();
        row.asset.chain = Some(ChainId(chain.into()));
        row
    };
    let b = parsed(
        &stable["network"],
        gate_base["name"].as_str().unwrap(),
        "base",
    );
    let e = parsed(
        &json!({"currency":"USDT","chains":[gate_eth]}),
        gate_eth["name"].as_str().unwrap(),
        "ethereum",
    );
    let quotes = [(cex.clone(), "USDT".to_string())].into_iter().collect();
    let nets = [(cex.clone(), Arc::new(vec![b.clone(), e.clone()]))]
        .into_iter()
        .collect();
    let chain_usdt = |net: &Networks, chain: &str| {
        crate::core::registry_build::usdt_address_in(net, &quotes, Some(&ChainId(chain.into())))
            .ok()
    };
    assert_eq!(chain_usdt(&nets, "base"), None);
    assert_eq!(chain_usdt(&nets, "ethereum"), Some(usdt));
    assert_eq!(address(pinned(&core, &base, &nets), None), Some(usdc));
    assert_eq!(address(pinned(&core, &base, &nets), Some(usdt)), Some(usdc));
    assert_eq!(
        address(pinned(&core, &eth, &nets), chain_usdt(&nets, "ethereum")),
        Some(usdt)
    );
    *wr(&core.inner.networks) = nets.clone();
    let market = crate::core::settlement_market::Market::new(&core, &base, &cex).unwrap();
    let mut native_row = parsed(
        &providers["native"],
        gate_base["name"].as_str().unwrap(),
        "base",
    );
    assert!(native_row.asset.contract.is_none());
    let wrapped = pool["sushiswap-v3-base"]["pool"]["tokens"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    native_row.asset.contract = Some(wrapped);
    let eth_asset = native_row.asset.clone();
    let wrapped_token = Asset {
        ticker: "WETH".into(),
        ..eth_asset.clone()
    };
    assert!(
        crate::core::settlement_network::erc20(
            &core,
            &[native_row],
            &cex,
            &market.cex_network,
            &wrapped_token
        )
        .is_err()
    );
    assert_eq!(market.balance_key(&core, &eth_asset, &base), "ETH");
    assert_eq!(
        market.balance_key(&core, &wrapped_token, &base),
        format!("{wrapped:#x}")
    );
    let stable_eth = e.asset.clone();
    for withdraw in [true, false] {
        assert!(crate::core::net_ready::direct_cash(&core, &cex, &stable_eth, withdraw).is_ok());
    }
    let eth_only = [(cex.clone(), Arc::new(vec![e]))].into_iter().collect();
    assert_eq!(address(pinned(&core, &base, &eth_only), Some(usdt)), None);
    let base_only = [(cex.clone(), Arc::new(vec![b]))].into_iter().collect();
    assert_eq!(
        address(
            pinned(&core, &eth, &base_only),
            chain_usdt(&base_only, "ethereum")
        ),
        None
    );
    let at = crate::settings::read::At {
        venue: Some(&cex.0),
        coin: None,
    };
    let old = gate_eth["name"].clone();
    s.set(
        "venue.ethereum_network",
        scope(&cex),
        Some(gate_base["name"].clone()),
        "test",
    )
    .await
    .unwrap();
    for chain in ["base", "ethereum"] {
        assert!(crate::gateway::own_nets::name_of(s, at, Some(&ChainId(chain.into()))).is_err());
    }
    assert_eq!(address(pinned(&core, &base, &nets), Some(usdt)), None);
    s.set("venue.ethereum_network", scope(&cex), Some(old), "test")
        .await
        .unwrap();
    *wr(&core.inner.networks) = base_only;
    for withdraw in [true, false] {
        assert!(crate::core::net_ready::direct_cash(&core, &cex, &stable_eth, withdraw).is_err());
    }
    s.set("venue.enabled", scope(&cex), Some(json!(false)), "test")
        .await
        .unwrap();
    assert_eq!(address(pinned(&core, &base, &nets), Some(usdt)), None);
}
