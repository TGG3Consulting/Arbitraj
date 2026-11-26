//! Сопоставляет DMM и Elastic с ответами соответствующих официальных роутеров на одном блоке.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::{dmm, swap_elastic};
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use serde_json::Value;
use std::collections::BTreeMap;

fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn elastic_math_matches_all_full_single_hops() {
    for (snapshot, key) in [
        (
            include_str!("data/first_run_dex_2026-11-26/kyber_base.json"),
            "kyberswap-elastic-base",
        ),
        (
            include_str!("data/first_run_dex_2026-11-26/kyber_ethereum.json"),
            "kyberswap_elastic",
        ),
    ] {
        let doc: Value = serde_json::from_str(snapshot).unwrap();
        let pools = doc["groups"][key]["pools"].as_array().unwrap();
        assert_eq!(pools.len(), 3);
        for p in pools {
            let mut ticks = BTreeMap::new();
            for t in p["ticks"].as_array().unwrap() {
                let gross: u128 = t["gross"].as_str().unwrap().parse().unwrap();
                if gross > 0 {
                    let net = t["net"].as_str().unwrap().parse().unwrap();
                    ticks.insert(t["tick"].as_i64().unwrap() as i32, TickData { gross, net });
                }
            }
            let map = TickMap {
                spacing: p["spacing"].as_i64().unwrap() as i32,
                ticks: ticks.into(),
                words: (i16::MIN, i16::MAX),
            };
            let state = swap_elastic::ElasticState {
                sqrt_p: number(&p["price"]),
                current_tick: p["tick"].as_i64().unwrap() as i32,
                base_l: p["liquidity"][0].as_str().unwrap().parse().unwrap(),
                reinvest_l: p["liquidity"][1].as_str().unwrap().parse().unwrap(),
                fee_units: p["fee"].as_u64().unwrap() as u32,
                ticks: &map,
                v1: false,
            };
            let quotes = p["quotes"].as_array().unwrap();
            assert_eq!(quotes.len(), 4);
            for q in quotes {
                let amount = number(&q["dx"]);
                let out = swap_elastic::swap(&state, q["i"] == 0, true, amount).unwrap();
                assert_eq!(
                    (out.amount_in, out.amount_out),
                    (amount, number(&q["out"])),
                    "{}",
                    p["pool"]
                );
            }
        }
    }
}

#[test]
fn dmm_dynamic_and_static_fee_match_their_routers() {
    let doc: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/kyber_ethereum.json"
    ))
    .unwrap();
    for key in [
        "kyberswap_classic_ethereum",
        "kyberswap_classic_ethereum-static",
    ] {
        let mut full = 0;
        for p in doc["groups"][key]["pools"].as_array().unwrap() {
            let trade = &p["trade"];
            let quotes = p["quotes"].as_array().unwrap();
            for q in quotes {
                let i = q["i"].as_u64().unwrap() as usize;
                let j = 1 - i;
                let side = dmm::Side {
                    r_in: number(&trade[i]),
                    r_out: number(&trade[j]),
                    v_in: number(&trade[2 + i]),
                    v_out: number(&trade[2 + j]),
                };
                assert_eq!(
                    dmm::amount_out(number(&q["dx"]), side, number(&trade[4])),
                    Some(number(&q["out"]))
                );
            }
            if quotes.len() == 4 {
                full += 1;
            }
        }
        assert_eq!(full, 2);
    }
}
