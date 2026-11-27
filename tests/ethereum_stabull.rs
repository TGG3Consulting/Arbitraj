//! Первичный Ethereum-реестр Stabull, терминальные валюты и состояние обоих направлений.
#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;

use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
fn registered_ethereum_markets_keep_their_own_cash_and_freeze_state() {
    let archive =
        include_bytes!("data/first_run_dex_2026-11-26/stabull_ethereum_directory.json.gz");
    let directory: Value = serde_json::from_reader(GzDecoder::new(archive.as_slice())).unwrap();
    let profile = first_run_profiles::named(1, "stabull-ethereum").unwrap();
    let params = &profile["settings"];
    assert_eq!(profile["chain_id"], directory["chain"]);
    assert_eq!(params["dex.factory"], directory["factory"]["address"]);
    assert_eq!(
        params["dex.factory_runtime_hash"],
        directory["factory"]["runtime"]
    );
    assert_eq!(params["dex.quoter"], directory["factory"]["router"]);
    assert_eq!(
        params["dex.quoter_runtime_hash"],
        directory["factory"]["router_runtime"]
    );
    let pools = directory["pools"].as_array().unwrap();
    assert_eq!(
        pools.len() as u64 + 1,
        directory["factory"]["nonce"].as_u64().unwrap()
    );
    let mut markets = BTreeSet::new();
    let mut funded_quotes = BTreeSet::new();
    let mut frozen = 0;
    for pool in pools {
        let market = pool["address"].as_str().unwrap();
        assert!(markets.insert(market));
        assert_eq!(pool["registered"], market);
        assert_eq!(pool["bound_token0"], pool["token0"]);
        assert_eq!(pool["bound_token1"], pool["token1"]);
        assert_eq!(pool["token1"], "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
        assert!(pool["rate0"].as_u64().unwrap() > 0);
        assert!(pool["rate1"].as_u64().unwrap() > 0);
        if pool["frozen"] == true {
            frozen += 1;
            assert_eq!(pool["pool_name"], "poundtoken (ETH-V1)");
        }
        if pool["quotes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|q| q["received"].as_u64().is_some_and(|n| n > 0))
        {
            funded_quotes.insert(pool["pool_name"].as_str().unwrap());
        }
    }
    assert_eq!((markets.len(), frozen), (7, 1));
    assert_eq!(
        funded_quotes,
        BTreeSet::from(["GYEN (ETH-V1)", "NZDS (ETH-V1)"])
    );
}

#[test]
fn protected_ethereum_payments_respect_the_wallet_and_both_limits() {
    let archive =
        include_bytes!("data/first_run_dex_2026-11-26/stabull_ethereum_protected_wallets.json.gz");
    let book: Value = serde_json::from_reader(GzDecoder::new(archive.as_slice())).unwrap();
    let profile = first_run_profiles::named(1, "stabull-ethereum").unwrap();
    assert_eq!(
        profile["settings"]["dex.module_runtime_hashes"][0],
        book["module"]["runtime"]
    );
    let mut paid = [0; 2];
    for market in book["records"].as_array().unwrap() {
        for case in market["cases"].as_array().unwrap() {
            let v5 = case["mode"] == "v5";
            for name in ["buy", "sell", "buy_target", "sell_target"] {
                let leg = &case[name];
                if leg["stage"] != "paid" {
                    continue;
                }
                let units = |v: &Value| v.as_str().unwrap().parse::<u128>().unwrap();
                let before = leg["wallet_and_module_before"].as_array().unwrap();
                let after = leg["wallet_and_module_after"].as_array().unwrap();
                assert_eq!(units(&before[0]) - units(&after[0]), units(&leg["paid"]));
                assert_eq!(units(&after[1]) - units(&before[1]), units(&leg["net"]));
                assert_eq!(before[2..], after[2..]);
                assert_eq!(leg["min_or_cap_error"]["code"], 3);
                assert_eq!(leg["deadline_error"]["code"], 3);
                assert!(leg["approval_gas"].as_u64().unwrap() > 0);
                if v5 {
                    assert!(leg["deadline_equality_gas"].as_u64().unwrap() > 0);
                }
                paid[usize::from(v5)] += 1;
            }
            if market["pool"] == "0x01e4013c478d7f02112c3cf178f2771c842edbd0"
                && case["size_usdc"] == 1000
            {
                assert_eq!(case["buy"]["stage"], "swap reverted");
                assert!(
                    case["buy"]["quoted"]
                        .as_str()
                        .unwrap()
                        .parse::<u128>()
                        .unwrap()
                        > 0
                );
            }
        }
    }
    assert_eq!(paid, [12, 6]);
}
