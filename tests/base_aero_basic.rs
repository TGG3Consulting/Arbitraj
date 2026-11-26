//! Оба точных выхода Aero Basic против фактических выплат на закреплённом блоке Base.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::{solidly_v1_math, swap_solidly};
use serde_json::Value;

fn amount(x: &Value) -> U256 {
    x.as_str().unwrap().parse().unwrap()
}

#[test]
fn both_curves_match_v5_before_and_after_first_swap() {
    let data: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/aero_basic_base.json"
    ))
    .unwrap();
    let mut count = 0;
    for factory in data["factories"].as_array().unwrap() {
        for pool in factory["markets"].as_array().unwrap() {
            let fee = U256::from(pool["base_fee"].as_u64().unwrap());
            let reserves = [amount(&pool["reserves"][0]), amount(&pool["reserves"][1])];
            let dec = [amount(&pool["precision"][0]), amount(&pool["precision"][1])];
            let calc = |i: usize, dx: U256, r: [U256; 2]| {
                if factory["curve"] == "stable" {
                    swap_solidly::get_amount_out(dx, r[i], r[1 - i], dec[i], dec[1 - i], fee)
                } else {
                    solidly_v1_math::volatile_amount_out(dx, r[i], r[1 - i], fee)
                }
            };
            for case in pool["cases"].as_array().unwrap() {
                let side = usize::from(case["token_in"] != pool["tokens"][0]);
                let results = &case["results"];
                let qr = results["after_reserves"]["returnData"].as_str().unwrap();
                let raw = hex::decode(qr.trim_start_matches("0x")).unwrap();
                let after = [
                    U256::from_be_slice(&raw[..32]),
                    U256::from_be_slice(&raw[32..64]),
                ];
                let bought = amount(&case["bought"]);
                let sold = amount(&case["sold"]);
                assert_eq!(calc(side, amount(&case["amount"]), reserves), Some(bought));
                assert_eq!(calc(1 - side, bought, after), Some(sold));
                assert_eq!(
                    amount(&results["buy_out"]["returnData"])
                        - amount(&results["before_out"]["returnData"]),
                    bought
                );
                assert_eq!(
                    amount(&results["sell_out"]["returnData"])
                        - amount(&results["buy_in"]["returnData"]),
                    sold
                );
                assert_eq!(results["above_buy"]["status"], "0x0");
                assert_eq!(results["above_sell"]["status"], "0x0");
                count += 2;
            }
        }
    }
    assert_eq!(count, 16);
}
