//! Solidly V3: фактические выплаты, частичное списание и ABI пула на Ethereum и Base.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::swap_cl::{ClState, Version, swap_final};
use arb::venues::bsc_dex::pools::{
    exec_kind, solidly_v3,
    ticks::{TickData, TickMap},
};
use serde_json::Value;

fn number(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn accepted_runtime_spends_whole_input_or_rolls_back() {
    for (chain, full_count, partial_count, recording) in [
        (
            8453,
            12,
            0,
            include_str!("data/first_run_dex_2026-11-26/solidly_v3_executor_base.json"),
        ),
        (
            1,
            11,
            1,
            include_str!("data/first_run_dex_2026-11-26/solidly_v3_executor_ethereum.json"),
        ),
    ] {
        let snapshot: Value = serde_json::from_str(recording).unwrap();
        let expected = snapshot["runtime_keccak"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(solidly_v3::accepted_runtime(chain), Some(expected));
        let mut counts = (0, 0);
        for case in snapshot["cases"].as_array().unwrap() {
            let result = &case["results"];
            for call in ["unregistered", "bad_tokens", "expired", "too_high"] {
                assert_eq!(result[call]["status"], "0x0");
            }
            for call in ["module_input", "module_output", "pool_allowance"] {
                assert_eq!(number(&result[call]["returnData"]), U256::ZERO);
            }
            let spent = number(&result["original_in"]["returnData"])
                - number(&result["in_after"]["returnData"]);
            let received = number(&result["out_after"]["returnData"])
                - number(&result["original_out"]["returnData"]);
            if case["full"] == true {
                assert_eq!(result["execute"]["status"], "0x1");
                assert_eq!(
                    (spent, received),
                    (number(&case["dx"]), number(&case["out"]))
                );
                assert_eq!(number(&result["execute"]["returnData"]), received);
                counts.0 += 1;
            } else {
                assert_eq!(result["execute"]["status"], "0x0");
                let error = alloy_primitives::keccak256(b"PartialFill()");
                assert_eq!(
                    result["execute"]["error"]["data"],
                    format!("0x{}", hex::encode(&error[..4]))
                );
                assert_eq!((spent, received), (U256::ZERO, U256::ZERO));
                counts.1 += 1;
            }
        }
        assert_eq!(counts, (full_count, partial_count));
    }
}

#[test]
fn full_input_quote_matches_real_wallet_and_partial_input_is_rejected() {
    assert!(exec_kind::of("solidly_v3").is_none());
    let mut totals = (0, 0, 0);
    for bytes in [
        include_str!("data/first_run_dex_2026-11-26/solidly_v3_base.json"),
        include_str!("data/first_run_dex_2026-11-26/solidly_v3_ethereum.json"),
    ] {
        let snapshot: Value = serde_json::from_str(bytes).unwrap();
        for market in snapshot["markets"].as_array().unwrap() {
            let ticks = market["ticks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    (
                        row[0].as_i64().unwrap() as i32,
                        TickData {
                            net: row[1].as_str().unwrap().parse().unwrap(),
                            gross: row[2].as_str().unwrap().parse().unwrap(),
                        },
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let map = TickMap {
                ticks: ticks.into(),
                spacing: market["spacing"].as_i64().unwrap() as i32,
                words: (
                    market["words"][0].as_i64().unwrap() as i16,
                    market["words"][1].as_i64().unwrap() as i16,
                ),
            };
            let state = ClState {
                sqrt_price: number(&market["price"]),
                tick: market["tick"].as_i64().unwrap() as i32,
                fee: market["fee"].as_u64().unwrap() as u32,
                liquidity: market["liquidity"].as_str().unwrap().parse().unwrap(),
                ticks: &map,
                version: Version::V3,
            };
            for case in market["cases"].as_array().unwrap() {
                let (dx, used, out) = (
                    number(&case["dx"]),
                    number(&case["taken"]),
                    number(&case["out"]),
                );
                let forward = case["side"] == 0;
                let raw =
                    hex::decode(case["quote"].as_str().unwrap().trim_start_matches("0x")).unwrap();
                assert_eq!(
                    solidly_v3::quote_output(&raw, forward, dx),
                    (dx == used).then_some(out)
                );
                if case["results"].is_null() {
                    assert!(used < dx);
                    assert!(case["gas_error"].as_str().unwrap().contains("16777216"));
                    totals.2 += 1;
                    continue;
                }
                let results = &case["results"];
                let balance = |key: &str| number(&results[key]["returnData"]);
                assert_eq!(balance("in_before") - balance("in_after"), used);
                assert_eq!(balance("out_after") - balance("out_before"), out);
                assert_eq!(balance("remaining"), dx - used);
                for key in ["above", "expired", "wrong_callback"] {
                    assert_eq!(results[key]["status"], "0x0");
                }
                if dx == used {
                    let (trade, after) = swap_final(&state, forward, true, dx).unwrap();
                    assert!(trade.complete);
                    assert_eq!((trade.amount_in, trade.amount_out), (dx, out));
                    assert_eq!(after.sqrt_price, number(&case["quote_after"][0]));
                    totals.0 += 1;
                } else {
                    totals.1 += 1;
                }
            }
        }
    }
    assert_eq!(totals, (23, 1, 1));
}
