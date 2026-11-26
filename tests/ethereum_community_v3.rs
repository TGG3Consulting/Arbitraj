//! Реестр CommunityV3 на Ethereum и исполнения на stock/USDC на том же блоке.
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::Value;
use std::collections::BTreeSet;

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
    assert_eq!(data["tick_lens"], p["dex.tick_lens"]);
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
