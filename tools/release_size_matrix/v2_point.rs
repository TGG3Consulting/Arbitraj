use alloy_primitives::U256;
use arb::venues::bsc_dex::math::v2;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::{env, fs, str::FromStr};

fn raw(s: &Value) -> U256 {
    U256::from_str(s.as_str().unwrap()).unwrap()
}

#[test]
fn quote_on_original_reserves() {
    let input: Value = serde_json::from_slice(&fs::read(env::var("V2_INPUT").unwrap()).unwrap()).unwrap();
    let mut results = Vec::new();
    for case in input["cases"].as_array().unwrap() {
        let mut amounts = Vec::new();
        let sell = case["side"] == "sell";
        for leg in case["legs"].as_array().unwrap() {
            let mut value = raw(&leg["raw"]);
            let hops = leg["hops"].as_array().unwrap();
            for index in 0..hops.len() {
                let hop = &hops[if sell { index } else { hops.len() - index - 1 }];
                let fees = v2::fee_ratio(Decimal::from_str(hop["fee"].as_str().unwrap()).unwrap()).unwrap();
                let (a,b) = (raw(&hop["rin"]),raw(&hop["rout"]));
                let next = if sell { v2::amount_out(value,a,b,fees) } else { v2::amount_in(value,a,b,fees) };
                if let Some(next) = next { value = next } else { value = U256::ZERO; break; }
            }
            amounts.push(value.to_string());
        }
        results.push(json!({"k": case["k"], "raw_usdt": amounts}));
    }
    fs::write(env::var("V2_OUT").unwrap(),serde_json::to_vec(&results).unwrap()).unwrap();
}