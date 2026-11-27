//! Concave Ethereum. Адрес каждого клона, расчёт двух способов исполнения и предел чистой выплаты.
use super::*;
use crate::venues::bsc_dex::math::v2;
use flate2::read::GzDecoder;
use std::collections::{HashMap, HashSet};

fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn quantity(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn registered_clones_price_the_actual_wallet_in_both_wires() {
    let bytes = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/concave_ethereum_wallets.json.gz"
    );
    let archive: Value = serde_json::from_reader(GzDecoder::new(&bytes[..])).unwrap();
    let factory = "0x066a5cb7ddc6d55384e2f6ca13d5dd2cd2685cbd"
        .parse()
        .unwrap();
    let implementation = "0xe8e429cb147ca1b984eb4e43b8813e6ddc36db6c"
        .parse()
        .unwrap();
    let mut markets = HashMap::new();
    let hash: B256 = archive["clone_hash"].as_str().unwrap().parse().unwrap();
    for m in archive["markets"].as_array().unwrap() {
        let pair = (address(&m["tokens"][0]), address(&m["tokens"][1]));
        let (pool, runtime) = clone(factory, implementation, pair);
        assert_eq!(pool, address(&m["pool"]));
        assert_eq!(keccak256(runtime), hash);
        assert_ne!(clone(factory, Address::ZERO, pair).0, pool);
        assert_ne!(clone(factory, implementation, (pair.1, pair.0)).0, pool);
        assert_eq!(m["fee_bps"], 25);
        assert_eq!(m["reserves"], m["balances"]);
        assert!(markets.insert(pool, pair).is_none());
    }
    assert_eq!(markets.len(), 7);
    let mut groups = HashMap::new();
    let mut admitted = HashSet::new();
    for v in archive["cases"].as_array().unwrap() {
        let market = address(&v["pool"]);
        let source = address(&v["source"]);
        let side = usize::from(markets[&market].0 != source);
        let spent = quantity(&v["spent"]);
        let received = quantity(&v["received"]);
        let a = quantity(&v["reserves"][side]);
        let b = quantity(&v["reserves"][1 - side]);
        assert_eq!(
            v2::amount_out(spent, a, b, (U256::from(9975), U256::from(10000))),
            Some(received)
        );
        assert_eq!(quantity(&v["before"][0]) - quantity(&v["after"][0]), spent);
        assert_eq!(
            quantity(&v["after"][1]) - quantity(&v["before"][1]),
            received
        );
        assert!(v["strict"].is_object() && v["expired"].is_object());
        admitted.insert((market, source));
        let group = groups
            .entry((market, source, spent, received))
            .or_insert_with(HashSet::new);
        assert!(group.insert(v["wire"].as_str().unwrap()));
    }
    assert!(admitted.len() >= 13);
    assert!(
        groups
            .values()
            .all(|m| m.contains("v5") && m.contains("router"))
    );
}
