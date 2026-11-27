//! Архивные проверки pairprice, точности атомов и физического актива для price.rs.
use super::*;
#[test]
fn published_pairprice_observations_preserve_the_fixed_side_and_name_the_native_alias() {
    use serde_json::Value;
    let j: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/dexalot_base_indicative.json"
    ))
    .unwrap();
    let pairs = Pairs(serde_json::from_value(j["pairs"].clone()).unwrap());
    let weth = pairs.0["WETH/USDC"].base_address;
    let mut outcomes = [0usize; 3];
    for row in j["prices"].as_array().unwrap() {
        let params = &row["params"];
        let data = row.get("retry").unwrap_or(&row["answer"]);
        if data["success"] != true {
            outcomes[2] += 1;
            continue;
        }
        let pair = &pairs.0[params["pair"].as_str().unwrap()];
        let sell = params["side"] == "1";
        let fixed_base = params["isbase"] == "1";
        let (input, output) = if sell {
            (pair.base_address, pair.quote_address)
        } else {
            (pair.quote_address, pair.base_address)
        };
        let dec = if fixed_base {
            pair.base_decimals
        } else {
            pair.quote_decimals
        };
        let qty = num::exact_amount(None, params["amount"].as_str(), dec).unwrap();
        let q = Query {
            input,
            output,
            qty,
            by_target: sell != fixed_base,
        };
        let ans = decode(
            &pairs,
            pair,
            q,
            serde_json::from_value(data.clone()).unwrap(),
            Some(weth),
        )
        .unwrap();
        let alias = ans.physical != pair;
        outcomes[usize::from(alias)] += 1;
        assert_eq!(if q.by_target { ans.net } else { ans.spent }, qty);
        if alias {
            assert_eq!(
                (pair.base_address, ans.physical.base_address),
                (weth, Address::ZERO)
            );
        }
    }
    assert!(outcomes.iter().all(|&n| n > 0));
}
#[test]
fn atoms_keep_exact_fixed_leg_and_round_variable_cash_in_the_owners_direction() {
    assert_eq!(human(U256::from(1_000_001u64), 6), "1.000001");
    assert_eq!(rounded("1.66623298", 6, true), Some(U256::from(1_666_233)));
    assert_eq!(rounded("1.66623298", 6, false), Some(U256::from(1_666_232)));
    assert_eq!(rounded("0.00000001", 6, true), Some(U256::ONE));
    assert_eq!(rounded("-0.123", 2, true), None);
}
