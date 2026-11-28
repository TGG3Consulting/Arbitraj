//! Сопоставление Curve IBT/PT со всеми публичными get_dy Spectra при реальных rates, fee и остатках.
mod curve_crypto_common;
mod multi_common;
#[path = "base_spectra_math/routing.rs"]
mod routing;

use alloy_primitives::{Address, U256};
use arb::core::types::CryptoData;
use arb::venues::bsc_dex::{
    math::{crypto_swap, stableswap_ng},
    pools::load_crypto_v1,
};
use serde_json::Value;

fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

fn crypto_state(r: &Value, pool: Address, now: u64) -> Option<CryptoData> {
    let got = curve_crypto_common::answers(r, &load_crypto_v1::calls_with(pool, 2, false));
    let decimals = r["decimals"]
        .as_array()?
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let snapshot = load_crypto_v1::Raw {
        pool,
        n: 2,
        got,
        now,
        decimals,
        killed: false,
    };
    load_crypto_v1::decode(&snapshot).ok().map(|(_, s)| s)
}

fn stable(r: &Value, i: usize, input: U256) -> Option<U256> {
    let calls = &r["calls"];
    let rates: Vec<_> = calls["stored_rates()"]
        .as_array()?
        .iter()
        .map(num)
        .collect();
    let balances: Vec<_> = r["balances"].as_array()?.iter().map(num).collect();
    stableswap_ng::exchange_received(
        i,
        1 - i,
        input,
        &balances,
        &rates,
        num(&calls["A_precise()"]),
        num(&calls["fee()"]),
        num(&calls["offpeg_fee_multiplier()"]),
    )
    .map(|x| x.0)
}

#[test]
fn spectra_ibts_and_pts_match_all_canonical_steps() {
    let raw = include_bytes!("data/first_run_dex_2026-11-26/spectra_base_math.json.gz");
    let book: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
    let mut totals = [0usize; 2];
    for r in book["markets"].as_array().unwrap() {
        let pool: Address = r["pool"].as_str().unwrap().parse().unwrap();
        let crypto = (r["kind"] == "legacy")
            .then(|| crypto_state(r, pool, book["timestamp"].as_u64().unwrap()));
        for q in r["quotes"].as_array().unwrap() {
            let i = q["i"].as_u64().unwrap() as usize;
            let dx = num(&q["dx"]);
            let actual = match &crypto {
                Some(data) => data
                    .as_ref()
                    .and_then(|s| crypto_swap::exchange_out(s, i, 1 - i, dx)),
                None => stable(r, i, dx),
            };
            let expected = q["dy"].as_str().map(|s| s.parse::<U256>().unwrap());
            assert_eq!(actual, expected, "{pool}, i={i}, input={dx}");
            totals[usize::from(expected.is_some())] += 1;
        }
    }
    assert_eq!(totals, [88, 216]);
}

#[test]
fn crypto_get_dy_matches_between_anchors_and_drops_on_adjacent_inputs() {
    use std::collections::{BTreeMap, BTreeSet};
    let gz = |raw: &[u8]| -> Value {
        serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap()
    };
    let book = gz(include_bytes!(
        "data/first_run_dex_2026-11-26/spectra_base_math.json.gz"
    ));
    let directory = gz(include_bytes!(
        "data/first_run_dex_2026-11-26/spectra_base_directory.json.gz"
    ));
    let intermediate = gz(include_bytes!(
        "data/first_run_dex_2026-11-26/spectra_base_inverse_intermediate.json.gz"
    ));
    let neighbors: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/spectra_base_inverse_neighbors.json"
    ))
    .unwrap();
    let adjacent: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/spectra_base_inverse_adjacent.json"
    ))
    .unwrap();
    let selector = &directory["selector"];
    let method = "get_dy(uint256,uint256,uint256)";
    assert_eq!(directory["chain_id"], 8453);
    assert_eq!(directory["block"], 62_262_756);
    assert_eq!(directory["timestamp"], book["timestamp"]);
    assert_eq!(&book["selector"], selector);
    assert_eq!(selector["requireCanonical"], true);
    assert_eq!(
        selector["blockHash"],
        "0xdf458884e3b25319497f122bfe50a2dff59b0079421fec1e1a9ac403d3c3e715"
    );
    for archive in [&intermediate, &neighbors] {
        assert_eq!(archive["block"], directory["block"]);
        assert_eq!(&archive["selector"], selector);
        assert_eq!(archive["method"], method);
    }
    let markets = book["markets"].as_array().unwrap();
    let mut states = BTreeMap::new();
    for m in directory["markets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "legacy")
    {
        let pool = m["pool"].as_str().unwrap();
        assert_eq!(m["factory"], m["own_factory"]);
        let source = markets.iter().find(|s| s["pool"] == pool).unwrap();
        assert_eq!(source["template"], m["template"]);
        if let Some(state) = crypto_state(
            source,
            pool.parse().unwrap(),
            book["timestamp"].as_u64().unwrap(),
        ) {
            states.insert(pool, state);
        }
    }
    let exact =
        |pool: &str, i: usize, dx| crypto_swap::exchange_out(&states[pool], i, 1 - i, dx).unwrap();
    let mut inputs = BTreeSet::new();
    let mut counts = BTreeMap::new();
    for point in intermediate["points"].as_array().unwrap() {
        let pool = point[0].as_str().unwrap();
        let i = point[1].as_u64().unwrap() as usize;
        let dx = num(&point[2]);
        let dy = num(&point[3]);
        assert!(dy > U256::ZERO && inputs.insert((pool, i, dx)));
        assert_eq!(exact(pool, i, dx), dy, "{pool}, i={i}, dx={dx}");
        *counts.entry((pool, i)).or_insert(0) += 1;
    }
    assert_eq!((inputs.len(), counts.len()), (92, 46));
    assert!(counts.values().all(|n| *n == 2));
    let pool = neighbors["pool"].as_str().unwrap();
    let i = neighbors["inputIndex"].as_u64().unwrap() as usize;
    let center = num(&neighbors["rawInput"]);
    let mut near = BTreeMap::new();
    for row in neighbors["samples"].as_array().unwrap() {
        let (dx, dy) = (num(&row[0]), num(&row[1]));
        assert_eq!(exact(pool, i, dx), dy, "{pool}, i={i}, dx={dx}");
        assert!(near.insert(dx, dy).is_none());
    }
    assert_eq!(near.len(), 23);
    assert_eq!(near[&center], num(&neighbors["target"]));
    let request = &adjacent["request"];
    let dx = num(&adjacent["rawInput"]);
    assert_eq!(dx, center + U256::from(3));
    assert_eq!(adjacent["block"], directory["block"]);
    assert_eq!(request["method"], "eth_call");
    assert_eq!(&request["params"][1], selector);
    assert_eq!(request["params"][0]["to"], pool);
    let sig = alloy_primitives::keccak256(method.as_bytes());
    let data = format!(
        "0x{}{:064x}{:064x}{:064x}",
        hex::encode(&sig[..4]),
        i,
        1 - i,
        dx
    );
    assert_eq!(request["params"][0]["data"].as_str(), Some(data.as_str()));
    let dy = U256::from_str_radix(
        adjacent["response"]["result"]
            .as_str()
            .unwrap()
            .strip_prefix("0x")
            .unwrap(),
        16,
    )
    .unwrap();
    assert_eq!(exact(pool, i, dx), dy);
    assert_eq!(near[&(dx - U256::from(1))], dy + U256::from(42));
}
