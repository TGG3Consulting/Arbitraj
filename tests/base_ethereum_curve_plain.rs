//! Сопоставляет целочисленную формулу Curve Plain с закреплёнными get_dy двух сетей.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t};
use serde_json::Value;
use std::str::FromStr;

fn integer(value: &Value) -> U256 {
    U256::from_str(value.as_str().unwrap()).unwrap()
}

#[test]
fn plain_factory_markets_match_dy() {
    for src in [
        include_str!("data/first_run_dex_2026-11-26/curve_plain_base.json"),
        include_str!("data/first_run_dex_2026-11-26/curve_plain_ethereum.json"),
    ] {
        let doc: Value = serde_json::from_str(src).unwrap();
        let pools = doc["pools"].as_array().unwrap();
        assert_eq!(pools.len(), 4);
        for p in pools {
            let balances: Vec<_> = p["balances"]
                .as_array()
                .unwrap()
                .iter()
                .map(integer)
                .collect();
            let rates: Vec<_> = p["decimals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| U256::from(10u8).pow(U256::from(36 - d.as_u64().unwrap())))
                .collect();
            let t = Template {
                amp_precision: U256::from(100u8),
                max_rounds: 255,
                strict: true,
                pair_d: balances.len() == 2,
            };
            for q in p["quotes"].as_array().unwrap() {
                let out = exchange_out_t(
                    q["i"].as_u64().unwrap() as usize,
                    q["j"].as_u64().unwrap() as usize,
                    integer(&q["dx"]),
                    &balances,
                    &rates,
                    integer(&p["a"]),
                    integer(&p["fee"]),
                    &t,
                );
                assert_eq!(out, Some(integer(&q["dy"])), "{}", p["pool"]);
            }
        }
    }
}
