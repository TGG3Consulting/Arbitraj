//! Проверяет три пула ListaV3 по состоянию, картам TickLens, реальным выплатам и неполному входу.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::swap_cl::{ClState, Version, swap_final};
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::virtual_quoter;
use serde_json::Value;
use std::collections::BTreeMap;

fn number(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn lista_v3_official_pool_outputs_and_partial_input() {
    let snapshot: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/lista_ethereum.json"
    )).unwrap();
    let pools = snapshot["markets"].as_array().unwrap();
    assert_eq!(pools.len(), 3);
    let mut partial = 0;
    for p in pools {
        let mut ticks = BTreeMap::new();
        let rows = p["words"].as_object().unwrap();
        let words: Vec<i16> = rows.keys().map(|s| s.parse().unwrap()).collect();
        for word in rows.values() {
            for row in word.as_array().unwrap() {
                let tick = row[0].as_i64().unwrap() as i32;
                let net = row[1].as_str().unwrap().parse().unwrap();
                let gross = row[2].as_str().unwrap().parse().unwrap();
                ticks.insert(tick, TickData { net, gross });
            }
        }
        let map = TickMap {
            words: (*words.iter().min().unwrap(), *words.iter().max().unwrap()),
            ticks: ticks.into(),
            spacing: p["spacing"].as_i64().unwrap() as i32,
        };
        let state = ClState {
            sqrt_price: number(&p["price"]),
            tick: p["tick"].as_i64().unwrap() as i32,
            liquidity: p["liquidity"].as_str().unwrap().parse().unwrap(),
            fee: p["fee"].as_u64().unwrap() as u32,
            ticks: &map,
            version: Version::V3,
        };
        let cases = p["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 4);
        for c in cases {
            let dx = number(&c["dx"]);
            let out = number(&c["out"]);
            let (swap, after) = swap_final(&state, c["source"] == p["tokens"][0], true, dx).unwrap();
            assert!(swap.complete);
            assert_eq!((swap.amount_in, swap.amount_out), (dx, out));
            assert_eq!(after.sqrt_price, number(&c["quote"][1]));
            let r = &c["results"];
            assert_eq!(r["above"]["status"], "0x0");
            let returned = |key: &str| number(&r[key]["returnData"]);
            assert_eq!(returned("in_before") - returned("in_after"), dx);
            assert_eq!(returned("out_after") - returned("out_before"), out);
            assert_eq!(returned("exact"), out);
        }
        for probe in p["virtual"].as_array().unwrap() {
            let dx = number(&probe["dx"]);
            if number(&probe["used"]) < dx {
                let raw = hex::decode(probe["response"].as_str().unwrap().trim_start_matches("0x")).unwrap();
                assert_eq!(virtual_quoter::decode(&raw, dx), Some(Err(virtual_quoter::SHORT)));
                assert_eq!(probe["out"], probe["native"][0]);
                partial += 1;
            }
        }
    }
    assert_eq!(partial, 2);
}
