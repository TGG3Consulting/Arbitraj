//! Квитанции обычного плательщика Ethereum. Котировка сверяется с движением
//! обоих активов, одним событием нужного пула и ценой, оставшейся после swap.
use alloy_primitives::{U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

fn units(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

fn check(data: &[u8], nmarkets: usize, wires: &[&str], active: usize) {
    let a: Value = serde_json::from_reader(GzDecoder::new(data)).unwrap();
    let markets = a["markets"].as_array().unwrap();
    assert_eq!(markets.len(), nmarkets);
    let signature = format!(
        "{:#x}",
        keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)")
    );
    let mut groups = HashMap::new();
    let mut directions = HashSet::new();
    for r in a["cases"].as_array().unwrap() {
        let pool = r["pool"].as_str().unwrap();
        let source = r["source"].as_str().unwrap();
        let m = markets.iter().find(|m| m["pool"] == pool).unwrap();
        let i = m["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .position(|t| t == source)
            .unwrap();
        assert_eq!(m["tokens"][1 - i], r["target"]);
        let spent = units(&r["spent"]);
        let received = units(&r["received"]);
        assert!(spent > U256::ZERO && received > U256::ZERO);
        assert_eq!(units(&r["before"][0]) - units(&r["after"][0]), spent);
        assert_eq!(units(&r["after"][1]) - units(&r["before"][1]), received);
        for k in [2, 3] {
            assert_eq!(r["before"][k], r["after"][k]);
        }
        assert_eq!(units(&r["quote"][0]), received);
        let mut swaps = r["logs"].as_array().unwrap().iter().filter(|e| {
            e["address"]
                .as_str()
                .is_some_and(|v| v.eq_ignore_ascii_case(pool))
                && e["topics"][0].as_str() == Some(signature.as_str())
        });
        let ev = swaps.next().unwrap();
        assert!(swaps.next().is_none());
        let hex = ev["data"].as_str().unwrap().strip_prefix("0x").unwrap();
        let word = |i: usize| U256::from_str_radix(&hex[i * 64..(i + 1) * 64], 16).unwrap();
        assert_eq!(word(i), spent);
        assert_eq!(word(1 - i), U256::ZERO.wrapping_sub(received));
        assert_eq!(word(2), units(&r["quote"][1]));
        directions.insert((pool, source));
        let b = groups
            .entry((pool, source, spent, received))
            .or_insert_with(HashSet::new);
        assert!(b.insert(r["wire"].as_str().unwrap()));
    }
    assert_eq!(directions.len(), active * 2);
    assert!(
        groups
            .values()
            .all(|v| v.len() == wires.len() && wires.iter().all(|w| v.contains(w)))
    );
}

#[test]
fn wagmi_actual_payouts_match_both_routers_executor_and_price() {
    check(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/wagmi_ethereum_wallets.json.gz"
        ),
        13,
        &["router", "router02", "v5"],
        12,
    );
}

#[test]
fn lif3_actual_payouts_match_router_executor_and_price() {
    check(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/lif3_ethereum_wallets.json.gz"
        ),
        6,
        &["router", "v5"],
        6,
    );
}
