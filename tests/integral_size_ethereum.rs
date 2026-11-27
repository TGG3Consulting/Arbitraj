//! Integral SIZE monetary outcomes for a disabled relayer, a queued sale, and historical atomic fills.
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::Value;
use std::collections::BTreeSet;

const DIRECTORY: &[u8] =
    include_bytes!("data/first_run_dex_2026-11-26/integral_size_ethereum.json.gz");
const WALLETS: &[u8] =
    include_bytes!("data/first_run_dex_2026-11-26/integral_size_ethereum_wallets.json.gz");

fn read(data: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(data)).unwrap()
}
fn scalar(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn balance(r: &Value) -> U256 {
    assert_eq!(r["status"], "0x1");
    scalar(&r["returnData"])
}
fn bytes(v: &Value) -> Bytes {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn full_registry_refuses_direct_pair_and_relayer_trades_while_queue_accepts_input_only() {
    let d = read(DIRECTORY);
    let w = read(WALLETS);
    let current = &w["current"];
    assert_eq!(d["selector"], current["selector"]);
    assert_eq!(d["state_root"], current["state_root"]);
    let pairs = d["markets"].as_array().unwrap();
    let mut unique = BTreeSet::new();
    assert_eq!(pairs.len(), 15);
    for market in pairs {
        let pair = market["pair"].as_str().unwrap();
        assert!(unique.insert(pair));
        assert_eq!(market["trader"], d["roles"]["delay"]["address"]);
        assert_eq!(market["atomic_enabled"], false);
        assert_eq!(market["delayed_sell_enabled"], true);
        let check = &market["direct_check"];
        assert_eq!(check["success"], false);
        assert_eq!(&bytes(&check["result"])[68..72], b"TP0C");
        for dir in [false, true] {
            let t = &market["tokens"];
            let (x, y) = if dir { (&t[1], &t[0]) } else { (&t[0], &t[1]) };
            for method in ["quoteSell", "quoteBuy"] {
                let matches: Vec<_> = d["atomic_checks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|c| {
                        c["pair"] == pair
                            && c["method"] == method
                            && &c["tokens"][0] == x
                            && &c["tokens"][1] == y
                    })
                    .collect();
                assert_eq!(matches.len(), 1);
                assert_eq!(matches[0]["success"], false);
                assert_eq!(&bytes(&matches[0]["result"])[68..72], b"TR5A");
            }
        }
    }
    assert_eq!(d["atomic_checks"].as_array().unwrap().len(), 60);
    let q = &current["queued"];
    let r = q["results"].as_array().unwrap();
    assert_eq!(r.len(), q["calls"].as_array().unwrap().len());
    assert_eq!(balance(&r[7]), balance(&r[3]) + U256::from(1));
    assert_eq!(balance(&r[8]), balance(&r[7]));
    assert_eq!(balance(&r[4]) - balance(&r[9]), scalar(&q["paid"]));
    assert_eq!(balance(&r[11]) - balance(&r[6]), scalar(&q["paid"]));
    assert_eq!(balance(&r[10]), balance(&r[5]));
    assert_eq!(scalar(&q["prepaid_wei"]), scalar(&q["calls"][7]["value"]));
    for attempt in current["rejections"].as_array().unwrap() {
        let r = attempt["results"].as_array().unwrap();
        assert_eq!(r[4]["status"], "0x0");
        assert_eq!(&bytes(&r[4]["error"]["data"])[68..72], b"TR5A");
        for i in 0..4 {
            assert_eq!(balance(&r[i]), balance(&r[i + 5]));
        }
    }
}

#[test]
fn earlier_weth_usdt_fills_pay_the_user_atomically_and_replenish_the_relayer_later() {
    let w = read(WALLETS);
    let h = &w["historical"];
    let mut unique = BTreeSet::new();
    for trade in h["cases"].as_array().unwrap() {
        let r = trade["results"].as_array().unwrap();
        let calls = trade["calls"].as_array().unwrap();
        assert_eq!(r.len(), calls.len());
        let output = scalar(&trade["net"]);
        let input = scalar(&trade["paid"]);
        let sell = trade["side"] == "sell";
        assert!(unique.insert((trade["input"].as_str().unwrap(), sell, output)));
        assert_eq!(balance(&r[6]), if sell { output } else { input });
        assert_eq!(r[7]["status"], "0x0");
        let expected = if sell { b"TR37" } else { b"TR08" };
        assert_eq!(&bytes(&r[7]["error"]["data"])[68..72], expected);
        assert_eq!(r[8]["status"], "0x0");
        assert_eq!(&bytes(&r[8]["error"]["data"])[68..72], b"OS04");
        assert_eq!(balance(&r[9]), U256::from(trade["order"].as_u64().unwrap()));
        assert_eq!(balance(&r[2]) - balance(&r[10]), input);
        assert_eq!(balance(&r[11]) - balance(&r[3]), output);
        assert_eq!(balance(&r[12]) - balance(&r[4]), input);
        assert_eq!(balance(&r[5]) - balance(&r[13]), output);
    }
    assert_eq!(unique.len(), 8);
    let d = read(DIRECTORY);
    let old = &d["historical"];
    let swap = &old["swap"];
    let transfer = &old["payment"];
    let settlement = &old["settlement"];
    assert_eq!(transfer["transactionHash"], swap["transactionHash"]);
    assert_eq!(transfer["address"], old["output"]);
    assert_eq!(
        bytes(&transfer["topics"][0]).as_ref(),
        keccak256(b"Transfer(address,address,uint256)").as_slice()
    );
    let proxy: Address = d["roles"]["relayerProxy"]["address"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(&bytes(&transfer["topics"][1])[12..], proxy.as_slice());
    assert_eq!(transfer["topics"][2], swap["topics"][2]);
    assert_eq!(scalar(&transfer["data"]), scalar(&old["received_raw"]));
    assert_eq!(swap["topics"][3], settlement["topics"][1]);
    assert_ne!(swap["transactionHash"], settlement["transactionHash"]);
    assert!(scalar(&settlement["blockNumber"]) > scalar(&swap["blockNumber"]));
    assert!(old["lag_seconds"].as_u64().unwrap() >= d["delay_seconds"].as_u64().unwrap());
}
