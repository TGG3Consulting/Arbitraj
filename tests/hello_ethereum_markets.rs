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

#[test]
fn both_cash_assets_fill_across_two_sizes_and_all_three_exchanges_with_atomic_limits() {
    let archive: &[u8] =
        include_bytes!("data/first_run_dex_2026-11-26/hello_ethereum_wallets.json.gz");
    let d: Value = serde_json::from_reader(flate2::read::GzDecoder::new(archive)).unwrap();
    let dirs: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/hello_ethereum_directory.json"
    ))
    .unwrap();
    assert_eq!(dirs["selector"], d["selector"]);
    assert_eq!(dirs["state_root"], d["state_root"]);
    let mut all = 0;
    let mut cash = BTreeSet::new();
    let number = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    for market in d["settlements"].as_array().unwrap() {
        assert!(cash.insert(addr(&market["cash"])));
        let payer = addr(&market["payer"]);
        assert_ne!(payer, addr(&d["treasury"]));
        let mut modes = BTreeMap::new();
        for case in market["cases"].as_array().unwrap() {
            for leg in ["buy", "sell"] {
                let row = &case[leg];
                let rs = row["results"].as_array().unwrap();
                let calls = row["calls"].as_array().unwrap();
                let n = rs.len();
                assert_eq!(calls.len(), n);
                assert_eq!(rs[n - 7]["status"], "0x0");
                assert_eq!(rs[n - 6]["status"], "0x0");
                assert_eq!(rs[n - 5]["status"], "0x1");
                assert_eq!(addr(&calls[n - 5]["from"]), payer);
                let cash_in = number(&rs[n - 13]["returnData"]) - number(&rs[n - 4]["returnData"]);
                let cash_out = number(&rs[n - 3]["returnData"]) - number(&rs[n - 12]["returnData"]);
                assert_eq!(cash_in, number(&row["paid"]));
                assert_eq!(cash_in, number(&row["input"]));
                assert_eq!(cash_out, number(&row["net"]));
                assert!(row["full"].as_bool().unwrap());
                assert_eq!(rs[n - 11]["returnData"], rs[n - 2]["returnData"]);
                assert_eq!(rs[n - 10]["returnData"], rs[n - 1]["returnData"]);
                all += 1;
            }
            let k = number(&case["buy"]["input"]);
            let shape = (number(&case["buy"]["net"]), number(&case["sell"]["net"]));
            let table: &mut BTreeMap<_, _> = modes.entry(k).or_default();
            assert!(
                table
                    .insert(case["mode"].as_str().unwrap(), shape)
                    .is_none()
            );
        }
        assert_eq!(modes.len(), 2);
        for variants in modes.values() {
            assert_eq!(variants.len(), 3);
            assert_eq!(variants["v5"], variants["router"]);
            assert_eq!(variants["v5"], variants["router02"]);
        }
    }
    assert_eq!(cash.len(), 2);
    assert_eq!(all, 24);
}
