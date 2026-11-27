//! Реестр CommunityV3 на Ethereum и исполнения на stock/USDC на том же блоке.
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
fn separately_quoted_whole_token_sizes_cover_both_sides_of_every_registered_pool() {
    let depth: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/community_v3_depth.json"
    ))
    .unwrap();
    let directory: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/community_v3_ethereum.json"
    ))
    .unwrap();
    let proof: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/community_v3_storage.json"
    ))
    .unwrap();
    assert_eq!(depth["state_root"], proof["state_root"]);
    assert_eq!(depth["selector"], directory["selector"]);
    assert_eq!(
        depth["quoter"],
        directory["execution"]["virtual_quoter_keccak"]
    );
    let markets = depth["markets"].as_array().unwrap();
    let registered = directory["pools"].as_array().unwrap();
    assert_eq!(markets.len(), registered.len());
    assert!(depth["missing_decimals"].as_object().unwrap().is_empty());
    let (mut full, mut partial, mut zero, mut reverted) = (0, 0, 0, 0);
    let mut sides = [0usize; 3];
    for (market, registry) in markets.iter().zip(registered) {
        assert_eq!(market["pool"], registry["pool"]);
        assert_eq!(market["tokens"], registry["tokens"]);
        let quotes = market["quotes"].as_array().unwrap();
        assert_eq!(quotes.len(), 4);
        let mut directions = 0;
        for side in [0, 1] {
            let address = market["tokens"][side].as_str().unwrap();
            let decimals = depth["decimals"][address].as_u64().unwrap();
            let unit = U256::from(10).pow(U256::from(decimals));
            let mut sizes = BTreeSet::new();
            let mut whole = false;
            for q in quotes
                .iter()
                .filter(|q| q["side"].as_u64() == Some(side as u64))
            {
                let as_units = |key: &str| q[key].as_str().unwrap().parse::<U256>().unwrap();
                let amount = as_units("input");
                assert!(sizes.insert(amount));
                if q["error"].is_object() {
                    reverted += 1;
                    continue;
                }
                let used = as_units("used");
                assert!(used <= amount);
                if as_units("out").is_zero() {
                    zero += 1;
                } else {
                    assert!(used > U256::ZERO);
                    if used == amount {
                        whole = true;
                        full += 1;
                    } else {
                        partial += 1;
                    }
                }
            }
            assert_eq!(sizes, BTreeSet::from([unit, unit * U256::from(10)]));
            directions += usize::from(whole);
        }
        sides[directions] += 1;
    }
    assert_eq!((full, partial, zero, reverted), (343, 169, 1222, 2));
    assert_eq!(sides, [305, 71, 58]);
}

#[test]
fn registered_addresses_reproduce_and_wallets_settle_through_v5() {
    let data: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/community_v3_ethereum.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let p = &profiles
        .iter()
        .find(|p| p["network"] == "ethereum" && p["id"] == "communityv3")
        .unwrap()["settings"];
    assert_eq!(data["registry"], p["dex.factory"]);
    let official: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/community_v3_ethereum_router.json"
    ))
    .unwrap();
    let chain: Value =
        serde_json::from_str(include_str!("data/first_run_dex_2026-11-26/ethereum.json")).unwrap();
    assert_eq!(data["selector"], official["selector"]);
    assert_eq!(data["selector"], chain["selector"]);
    let lens = &official["roles"]["lens"];
    assert_eq!(p["dex.tick_lens"], lens["address"]);
    // Исторический эксперимент использовал независимое размещение того же неизменяемого TickLens.
    for addr in [&data["tick_lens"], &p["dex.tick_lens"]] {
        let key = addr.as_str().unwrap().to_ascii_lowercase();
        assert_eq!(chain["contracts"][&key]["bytes"], 1385);
        assert_eq!(chain["contracts"][&key]["keccak256"], lens["runtime"]);
    }
    for (name, key) in [
        ("factory", "dex.factory"),
        ("router", "dex.router"),
        ("quoter", "dex.quoter"),
    ] {
        let role = &official["roles"][name];
        assert_eq!(p[key], role["address"]);
        assert_eq!(
            chain["contracts"][role["address"].as_str().unwrap()]["keccak256"],
            role["runtime"]
        );
    }
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let uint = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let factory = addr(&data["registry"]);
    for entity in ["factory", "pool"] {
        assert_eq!(
            data["proof"][entity]["deployed"]["creation"],
            data["proof"][entity]["reference"]["creation"]
        );
    }
    let init: B256 = data["proof"]["pool"]["deployed"]["creation"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let markets = data["pools"].as_array().unwrap();
    let mut found = BTreeSet::new();
    for m in markets {
        let fee = m["fee"].as_u64().unwrap();
        assert!(data["tiers"][fee.to_string()]["spacing"].as_i64().unwrap() > 0);
        let salt = keccak256(
            (
                addr(&m["tokens"][0]),
                addr(&m["tokens"][1]),
                U256::from(fee),
            )
                .abi_encode(),
        );
        let pool = factory.create2(salt, init);
        assert_eq!(addr(&m["pool"]), pool);
        assert!(found.insert(pool));
    }
    let nonce = usize::from_str_radix(data["nonce"].as_str().unwrap().trim_start_matches("0x"), 16)
        .unwrap();
    assert_eq!(found.len() + 1, nonce);
    assert_eq!(found.len(), 434);
    assert_eq!(data["tiers"].as_object().unwrap().len(), 25);
    assert_eq!(
        p["dex.v3_fee_tiers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        data["tiers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    );
    let swaps = data["execution"]["pools"].as_array().unwrap();
    assert_eq!(swaps.len(), 3);
    for market in swaps {
        assert!(found.contains(&addr(&market["pool"])));
        let sizes = market["cases"].as_array().unwrap();
        assert_eq!(sizes.len(), 3);
        for t in sizes {
            assert!(uint(&t["bought"]) > U256::ZERO && uint(&t["sold"]) > U256::ZERO);
            assert_eq!(
                uint(&t["cash_before"]) - uint(&t["input"]) + uint(&t["sold"]),
                uint(&t["cash_after"])
            );
            assert_eq!(t["min_out_plus_one_buy_reverted"], true);
            assert_eq!(t["min_out_plus_one_sell_reverted"], true);
        }
    }
}
