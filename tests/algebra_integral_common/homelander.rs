//! Принадлежность денежного обмена в Base по двум реестрам плагинов и записям симуляций AMM.
use alloy_primitives::{Address, B256, U256, address, keccak256};
use serde_json::Value;
use std::collections::BTreeMap;

fn address_word(word: &Value) -> Address {
    Address::from_word(word.as_str().unwrap().parse::<B256>().unwrap())
}

fn raw(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

/// По одной записи на физический пул; повторное создание плагина не удваивает рынок.
fn compare_catalog(
    snapshot: &Value,
    profiles: &[Value],
    paid: &BTreeMap<String, (Address, Address)>,
) {
    let directory: Value = serde_json::from_str(include_str!(
        "../data/first_run_dex_2026-11-26/homelander_base_directory.json"
    ))
    .unwrap();
    assert_eq!(directory["selector"], snapshot["selector"]);
    assert_eq!(directory["chain_id"], snapshot["chain_id"]);
    let height = directory["height"].as_u64().unwrap();
    let events = directory["events"].as_array().unwrap();
    let mut pools = BTreeMap::new();
    for event in events {
        assert!(pools.insert(event["pool"].as_str().unwrap(), event).is_none());
    }
    let registries = directory["registries"].as_array().unwrap();
    assert_eq!(registries.len(), 2);
    for (i, expected) in [11, 32].into_iter().enumerate() {
        let registry = &registries[i];
        assert_eq!(registry["address"], snapshot["registries"][i]);
        let start = registry["from_block"].as_u64().unwrap();
        assert_eq!(registry["to_block"].as_u64(), Some(height));
        assert!(registry["cache_ranges"].as_array().unwrap().iter().any(|range| {
            range[0].as_u64().unwrap() <= start && range[1].as_u64().unwrap() >= height
        }));
        let observed = events.iter().filter(|e| e["registry"] == registry["address"]);
        assert_eq!(observed.clone().count(), expected);
        for event in observed {
            assert!((start..=height).contains(&event["block"].as_u64().unwrap()));
        }
    }
    let mut coverage = BTreeMap::new();
    let mut liquid = BTreeMap::new();
    for market in directory["markets"].as_array().unwrap() {
        let pool = market["pool"].as_str().unwrap();
        let event = pools.remove(pool).unwrap();
        let registry = event["registry"].as_str().unwrap();
        let plugin = market["active_plugin"].as_str().unwrap().parse::<Address>().unwrap();
        assert_ne!(plugin, Address::ZERO);
        assert_eq!(market["registries"][registry], event["plugin"]);
        assert_eq!(market["active_plugin"], event["plugin"]);
        assert_eq!(market["factory_lookup"]["poolByPair"], pool);
        let mut parents = profiles.iter().filter(|p| {
            p["network"] == "base" && p["settings"]["dex.factory"] == market["amm_factory"]
        });
        let parent = parents.next().unwrap()["id"].as_str().unwrap();
        assert!(parents.next().is_none());
        assert_eq!(market["known_parent_profile"], parent);
        if !raw(&market["liquidity_raw"]).is_zero() {
            *liquid.entry(parent).or_insert(0usize) += 1;
        }
        assert_eq!(market["global_state"]["unlocked"], true);
        if parent == "quickswap-v4-base" {
            let plugin_check = &market["quickswap_plugin_check"];
            assert_eq!(plugin_check["matches_adapter_checks"], true);
            assert_eq!(address_word(&plugin_check["embedded_word"]), pool.parse::<Address>().unwrap());
        }
        let counts = coverage.entry(parent).or_insert((0, 0));
        if let Some(&(factory, active)) = paid.get(pool) {
            assert_eq!(market["amm_factory"].as_str().unwrap().parse::<Address>().unwrap(), factory);
            assert_eq!(plugin, active);
            counts.0 += 1;
        } else {
            counts.1 += 1;
        }
        assert_eq!(market["previous_payment_fixture"], paid.contains_key(pool));
    }
    assert!(pools.is_empty());
    assert_eq!(liquid, BTreeMap::from([("hydrex-integral", 11), ("quickswap-v4-base", 19)]));
    assert_eq!(
        coverage,
        BTreeMap::from([("hydrex-integral", (3, 8)), ("quickswap-v4-base", (3, 29))])
    );
}

#[test]
fn registered_plugins_preserve_the_physical_pool_and_wallet_payout() {
    let snapshot: Value = serde_json::from_str(include_str!(
        "../data/first_run_dex_2026-11-26/homelander_base.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../../config/first_run_dex.json")).unwrap();
    let swap = format!(
        "{:#x}",
        keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24,uint24,uint24)")
    );
    // Исполнитель из chainConfig[BASE] первичного адаптера, закреплённого в snapshot.sources.
    let plugin_executor = address!("3a980817e1522c532cc504dba5f5fee9e9096ac5");
    let mut paid_pools = BTreeMap::new();
    for (profile, archive, expected_internal) in [
        (
            "hydrex-integral",
            include_str!("../data/first_run_dex_2026-11-26/hydrex_base.json"),
            6,
        ),
        (
            "quickswap-v4-base",
            include_str!("../data/first_run_dex_2026-11-26/quickswap_base.json"),
            2,
        ),
    ] {
        let amm: Value = serde_json::from_str(archive).unwrap();
        assert_eq!(amm["selector"], snapshot["selector"]);
        let settings = &profiles
            .iter()
            .find(|p| p["id"] == profile && p["network"] == "base")
            .unwrap()["settings"];
        let factory: Address = settings["dex.factory"].as_str().unwrap().parse().unwrap();
        let executor: Address = amm["executor"].as_str().unwrap().parse().unwrap();
        let mut internal = 0;
        let mut customer = 0;
        for market in amm["markets"].as_array().unwrap() {
            let pool = market["pool"].as_str().unwrap();
            let registry = snapshot["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["pool"] == pool)
                .unwrap();
            assert_eq!(registry["requests"][2]["to"], pool);
            assert_eq!(registry["requests"][3]["to"], pool);
            assert!(snapshot["registries"].as_array().unwrap().contains(&registry["registry"]));
            let answers = &registry["responses"];
            let plugin = address_word(&answers[2]["result"]);
            assert_eq!(address_word(&answers[3]["result"]), factory);
            assert_eq!(plugin, market["plugin"].as_str().unwrap().parse::<Address>().unwrap());
            assert_ne!(plugin, Address::ZERO);
            assert!(paid_pools.insert(pool.to_owned(), (factory, plugin)).is_none());
            for i in 0..2 {
                let request = &registry["requests"][i];
                assert_eq!(request["to"], snapshot["registries"][i]);
                let expected = if request["to"] == registry["registry"] {
                    plugin
                } else {
                    Address::ZERO
                };
                assert_eq!(address_word(&answers[i]["result"]), expected);
            }
            for case in market["cases"].as_array().unwrap() {
                let r = &case["results"];
                let owner: Address = case["sender"].as_str().unwrap().parse().unwrap();
                let balance = |k: &str| raw(&r[k]["returnData"]);
                let paid = balance("in_before") - balance("in_after");
                let received = balance("out_after") - balance("out_before");
                let debit_side = usize::from(case["source"] != market["tokens"][0]);
                let mut fills = 0;
                for log in r["exact"]["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|l| l["topics"][0] == swap)
                {
                    assert_eq!(log["address"], pool);
                    let sender = address_word(&log["topics"][1]);
                    let recipient = address_word(&log["topics"][2]);
                    if sender == plugin_executor {
                        assert_eq!(recipient, plugin_executor);
                        internal += 1;
                        continue;
                    }
                    assert_eq!(sender, executor);
                    assert_eq!(recipient, owner);
                    let data = log["data"].as_str().unwrap().strip_prefix("0x").unwrap();
                    let delta = |side: usize| {
                        U256::from_str_radix(&data[64 * side..64 * (side + 1)], 16).unwrap()
                    };
                    assert_eq!(delta(debit_side), paid);
                    assert_eq!(delta(1 - debit_side), U256::ZERO.wrapping_sub(received));
                    fills += 1;
                }
                assert_eq!(fills, 1);
                customer += fills;
            }
        }
        assert_eq!((customer, internal), (12, expected_internal));
    }
    assert_eq!(paid_pools.len(), snapshot["markets"].as_array().unwrap().len());
    compare_catalog(&snapshot, &profiles, &paid_pools);
}
