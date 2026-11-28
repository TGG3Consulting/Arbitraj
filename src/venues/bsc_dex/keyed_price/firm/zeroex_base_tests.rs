//! Сверка исторических переводов маркетмейкеров Base с маркерами RFQ и каноническими квитанциями.
use alloy_primitives::{Address, U256, keccak256};
use serde_json::Value;
use std::collections::BTreeSet;

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn amount(v: &str) -> U256 {
    U256::from_str_radix(v.trim_start_matches("0x"), 16).unwrap()
}

fn address(v: &str) -> Address {
    v.parse().unwrap()
}

fn transfer(log: &Value, tx: &Value, payer: Address, recipient: Address) -> U256 {
    assert_eq!(log["transactionHash"], tx["transaction"]);
    assert_eq!(log["blockHash"], tx["blockHash"]);
    assert_eq!(amount(text(&log["blockNumber"])), U256::from(tx["block"].as_u64().unwrap()));
    let topics = log["topics"].as_array().unwrap();
    assert_eq!(topics.len(), 3);
    assert_eq!(text(&topics[0]), format!("{:#x}", keccak256(b"Transfer(address,address,uint256)")));
    assert_eq!(amount(text(&topics[1])), U256::from_be_slice(payer.as_slice()));
    assert_eq!(amount(text(&topics[2])), U256::from_be_slice(recipient.as_slice()));
    let moved = amount(text(&log["data"]));
    assert!(!moved.is_zero());
    moved
}

#[test]
fn recorded_base_rfq_legs_match_their_own_transfers_and_usdt_search_windows() {
    let snapshot = archive(include_bytes!(
        "../../../../../tests/data/first_run_dex_2026-11-26/zeroex_base_payments.json.gz"
    ));
    let cash = archive(include_bytes!(
        "../../../../../tests/data/first_run_dex_2026-11-26/base_usdt_cycle.json.gz"
    ));
    let roles: Value = serde_json::from_str(include_str!(
        "../../../../../tests/data/first_run_dex_2026-11-26/zeroex_base_registry_roles.json"
    )).unwrap();
    let head = &roles["observations"][1];
    let height = snapshot["height"].as_u64().unwrap();
    assert_eq!(snapshot["chainId"], 8453);
    assert_eq!(snapshot["selector"], head["selector"]);
    assert_eq!(snapshot["height"], head["block"]);
    assert_eq!(snapshot["cash"], cash["cash"]);
    let settlers = snapshot["settlers"].as_array().unwrap();
    assert_eq!(settlers.len(), 2);
    for (label, settler) in ["current", "previous"].iter().zip(settlers) {
        assert_eq!(amount(text(&head[*label]["response"]["result"])), amount(text(settler)));
    }
    let mut hashes = BTreeSet::new();
    let mut makers = BTreeSet::new();
    let mut mark_count = 0;
    let mut leg_count = 0;
    let windows = snapshot["windows"].as_array().unwrap();
    let mut marks = vec![0_u64; windows.len()];
    let first = height + 1 - 43_200;
    for tx in snapshot["transactions"].as_array().unwrap() {
        assert!(hashes.insert(text(&tx["transaction"])));
        let number = tx["block"].as_u64().unwrap();
        assert_eq!(snapshot["canonicalBlocks"][number.to_string()], tx["blockHash"]);
        assert!((first..=height).contains(&number));
        assert!(!tx["tokens"].as_array().unwrap().contains(&snapshot["cash"]));
        let emitted = tx["markers"].as_array().unwrap();
        let mut positions = BTreeSet::new();
        for m in emitted {
            assert!(settlers.contains(&m["settler"]));
            assert!(positions.insert(m["logIndex"].as_u64().unwrap()));
            assert_eq!(text(&m["data"]).len(), 98);
        }
        mark_count += emitted.len();
        marks[((number - first) / 3600) as usize] += emitted.len() as u64;
        for leg in tx["legs"].as_array().unwrap() {
            let maker = address(text(&leg["maker"]));
            let settler = address(text(&leg["settler"]));
            makers.insert(maker);
            let marker = &leg["marker"];
            assert_eq!(marker["transactionHash"], tx["transaction"]);
            assert_eq!(marker["blockHash"], tx["blockHash"]);
            assert_eq!(marker["address"], leg["settler"]);
            assert!(marker["topics"].as_array().unwrap().is_empty());
            assert!(emitted.iter().any(|m| m["settler"] == marker["address"] && m["data"] == marker["data"]
                && U256::from(m["logIndex"].as_u64().unwrap()) == amount(text(&marker["logIndex"]))));
            let paid = &leg["makerInput"];
            let delivered = &leg["makerOutput"];
            assert_ne!(paid["address"], delivered["address"]);
            transfer(paid, tx, settler, maker);
            let size = transfer(delivered, tx, maker, settler);
            assert_eq!(amount(&text(&marker["data"])[66..]), size);
            let action = text(&leg["matchedCalldata"]);
            let limit = action.len() - 64;
            assert_eq!(&action[limit-40..limit], &text(&delivered["address"])[2..]);
            assert!(size <= amount(&action[limit..]));
            leg_count += 1;
        }
    }
    assert_eq!((mark_count, hashes.len(), leg_count, makers.len()), (118, 107, 92, 4));
    let reported: BTreeSet<_> = snapshot["makers"].as_array().unwrap().iter().map(|a| address(text(a))).collect();
    assert_eq!(makers, reported);
    assert_eq!(windows.len(), 12);
    for (i, window) in windows.iter().enumerate() {
        let start = first + i as u64 * 3600;
        assert_eq!(window, &serde_json::json!([start, start+3599, marks[i]]));
    }
    let scans = snapshot["makerCashTransfers"].as_array().unwrap();
    assert_eq!(scans.len(), 30);
    for (day, scan) in scans.iter().enumerate() {
        let end = height - 43_200 * day as u64;
        assert_eq!(scan, &serde_json::json!([end-43_199, end, [], []]));
    }
}
