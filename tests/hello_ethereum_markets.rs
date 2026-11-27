//! HELLO Ethereum. The deployer creates one pool for each nonce after initialization.
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn all_registered_markets_derive_from_deployer_and_threshold_payments_are_distinct() {
    let d: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/hello_ethereum_directory.json"
    ))
    .unwrap();
    let observed: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/hello_ethereum_swaps.json"
    ))
    .unwrap();
    assert_eq!(d["selector"], observed["selector"]);
    assert_eq!(d["state_root"], observed["state_root"]);
    let pools = d["markets"].as_array().unwrap();
    assert_eq!(pools.len() as u64 + 1, d["nonce"].as_u64().unwrap());
    let deployer = addr(&d["deployer"]);
    let init: B256 = d["init_hash"].as_str().unwrap().parse().unwrap();
    let mut registry = BTreeSet::new();
    for m in pools {
        let t0 = addr(&m["tokens"][0]);
        let t1 = addr(&m["tokens"][1]);
        assert!(t0 < t1);
        let fee = m["fee"].as_u64().unwrap();
        assert_eq!(d["tiers"][fee.to_string()], m["step"]);
        let mut preimage = [0u8; 96];
        preimage[..32].copy_from_slice(t0.into_word().as_slice());
        preimage[32..64].copy_from_slice(t1.into_word().as_slice());
        preimage[64..].copy_from_slice(&U256::from(fee).to_be_bytes::<32>());
        let pool = addr(&m["pool"]);
        assert_eq!(deployer.create2(keccak256(preimage), init), pool);
        assert!(registry.insert(pool));
    }
    let swap =
        keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)");
    let collect = keccak256(b"CollectProtocol(address,address,uint128,uint128)");
    let protocol = addr(&observed["protocol_fee_recipient"]);
    let mut per_pool: BTreeMap<Address, [u64; 2]> = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for e in observed["events"].as_array().unwrap() {
        assert_ne!(e["removed"], true);
        assert!(ids.insert((
            e["blockHash"].as_str().unwrap(),
            e["logIndex"].as_str().unwrap()
        )));
        let pool = addr(&e["address"]);
        assert!(registry.contains(&pool));
        let event: B256 = e["topics"][0].as_str().unwrap().parse().unwrap();
        let to: B256 = e["topics"][2].as_str().unwrap().parse().unwrap();
        let data: Bytes = serde_json::from_value(e["data"].clone()).unwrap();
        let counts = per_pool.entry(pool).or_default();
        if event == swap {
            assert_eq!(data.len(), 7 * 32);
            counts[0] += 1;
        } else {
            assert_eq!(event, collect);
            assert_eq!(data.len(), 2 * 32);
            assert_eq!(Address::from_slice(&to.as_slice()[12..]), protocol);
            counts[1] += 1;
        }
    }
    assert_eq!(per_pool.len(), 5);
    assert_eq!(per_pool.values().map(|n| n[0]).sum::<u64>(), 10);
    assert_eq!(per_pool.values().map(|n| n[1]).sum::<u64>(), 10);
    let transfers = observed["protocol_transfers"].as_array().unwrap();
    assert_eq!(transfers.len(), 10);
    for r in transfers {
        let l = &r["log"];
        assert_eq!(
            l["topics"][0].as_str().unwrap().parse::<B256>().unwrap(),
            keccak256(b"Transfer(address,address,uint256)")
        );
        let from = l["topics"][1].as_str().unwrap().parse::<B256>().unwrap();
        let to = l["topics"][2].as_str().unwrap().parse::<B256>().unwrap();
        assert!(registry.contains(&Address::from_slice(&from.as_slice()[12..])));
        assert_eq!(Address::from_slice(&to.as_slice()[12..]), protocol);
        let collection = observed["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| {
                event["transactionHash"] == l["transactionHash"]
                    && event["logIndex"] == r["collect_index"]
            })
            .unwrap();
        assert_eq!(collection["blockHash"], l["blockHash"]);
        assert_eq!(
            collection["topics"][0]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap(),
            collect
        );
    }
    let payouts = observed["swap_transfers"].as_array().unwrap();
    assert_eq!(payouts.len(), 10);
    for row in payouts {
        assert_eq!(row["quoted"], row["transferred"]);
        let sum = row["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                assert_eq!(l["transactionHash"], row["tx"]);
                assert_eq!(l["address"], row["asset"]);
                assert!(
                    transfers
                        .iter()
                        .all(|f| f["log"]["transactionHash"] != l["transactionHash"]
                            || f["log"]["logIndex"] != l["logIndex"])
                );
                l["data"].as_str().unwrap().parse::<U256>().unwrap()
            })
            .sum::<U256>();
        assert_eq!(
            sum,
            row["transferred"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap()
        );
    }
    assert_eq!(observed["swap_count"], 10);
}
