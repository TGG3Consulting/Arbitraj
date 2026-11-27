//! Сверяет холодное обнаружение ставки Alien Base и физические выплаты при одинаковых входах.
use alloy_primitives::U256;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{collections::BTreeMap, fs::File};

fn capture(file: &str) -> Value {
    let file = File::open(format!(
        "tests/data/first_run_dex_2026-11-26/{file}.json.gz"
    ))
    .unwrap();
    serde_json::from_reader(GzDecoder::new(file)).unwrap()
}

#[test]
fn registered_fee_is_discovered_and_its_wallet_output_competes_at_identical_size() {
    let profiles: Vec<Value> =
        serde_json::from_reader(File::open("config/first_run_dex.json").unwrap()).unwrap();
    let preset = &profiles
        .iter()
        .find(|p| p["network"] == "base" && p["id"] == "alien-base-v3")
        .unwrap()["settings"];
    let snap = capture("alien_fee_comparison_base");
    let index = capture("alien_fee_discovery_base");
    assert_eq!(snap["selector"], index["selector"]);
    assert_eq!(snap["selector"]["requireCanonical"], true);
    assert_eq!(preset["dex.factory"], snap["factory"]);
    let physical = &index["mixed_fee_probe"]["found"]["WETH/USDC"];
    let fees = preset["dex.v3_fee_tiers"].as_array().unwrap();
    let mut cash: BTreeMap<(&str, u128), BTreeMap<u64, u128>> = BTreeMap::new();
    for market in snap["markets"].as_array().unwrap() {
        let tier = market["tier"].as_u64().unwrap();
        let Some(attempts) = market["attempts"].as_array() else {
            continue;
        };
        assert_eq!(physical[tier.to_string()], market["pool"]);
        let tier_key = tier.to_string();
        assert!(fees.iter().any(|f| f.as_str() == Some(tier_key.as_str())));
        if tier == 750 {
            assert_eq!(market["spacing"], 15);
        }
        for a in attempts {
            let source = a["source"].as_str().unwrap();
            let leg = &a[if source == "WETH" { "buy" } else { "sell" }];
            if leg.is_null() {
                assert!(
                    a["reverted"]
                        .as_str()
                        .unwrap()
                        .contains("execution reverted")
                );
                continue;
            }
            let exact = &leg["exact"];
            assert_eq!(exact["status"], "0x1");
            assert_eq!(exact["spent"], a["size"]);
            for key in ["above", "expired"] {
                assert_eq!(leg[key]["status"], "0x0");
                assert_eq!(leg[key]["spent"], "0");
                assert_eq!(leg[key]["received"], "0");
            }
            let mut swaps = exact["logs"].as_array().unwrap().iter().filter(|event| {
                event["address"] == market["pool"]
                    && event["topics"][0]
                        == "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"
            });
            let swap = swaps.next().unwrap();
            assert!(swaps.next().is_none());
            let qty = a["size"].as_str().unwrap().parse::<u128>().unwrap();
            let net = exact["received"].as_str().unwrap().parse::<u128>().unwrap();
            let data = swap["data"].as_str().unwrap().strip_prefix("0x").unwrap();
            let words = [
                U256::from_str_radix(&data[..64], 16).unwrap(),
                U256::from_str_radix(&data[64..128], 16).unwrap(),
            ];
            let src = usize::from(source == "USDC");
            assert_eq!(words[src], U256::from(qty));
            assert_eq!(words[1 - src], U256::ZERO.wrapping_sub(U256::from(net)));
            assert!(
                cash.entry((source, qty))
                    .or_default()
                    .insert(tier, net)
                    .is_none()
            );
        }
    }
    assert_eq!(cash.len(), 4);
    for levels in cash.values() {
        assert_eq!(levels.len(), 3);
        assert!(levels[&750] > levels[&200].max(levels[&400]));
    }
}
