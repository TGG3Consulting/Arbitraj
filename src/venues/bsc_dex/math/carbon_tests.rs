use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn canonical_orders_match_both_controller_trade_methods_and_actual_wallet_fills() {
    let bytes =
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz");
    let data: Value = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
    let directory = data["directory"].as_array().unwrap();
    let mut checked = 0;
    for c in data["controls"].as_array().unwrap() {
        let source = c["source"].as_str().unwrap();
        let target = c["target"].as_str().unwrap();
        let pair = directory
            .iter()
            .find(|p| {
                p["tokens"].as_array().unwrap().iter().any(|v| v == source)
                    && p["tokens"].as_array().unwrap().iter().any(|v| v == target)
            })
            .unwrap();
        let strategy = pair["orders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v[0] == c["id"])
            .unwrap();
        let idx = strategy[2]
            .as_array()
            .unwrap()
            .iter()
            .position(|v| v == target)
            .unwrap();
        let side = &strategy[3][idx];
        let o = CarbonOrder {
            y: side[0].as_str().unwrap().parse().unwrap(),
            z: side[1].as_str().unwrap().parse().unwrap(),
            a: side[2].as_str().unwrap().parse().unwrap(),
            b: side[3].as_str().unwrap().parse().unwrap(),
        };
        let x: u128 = c["input"].as_str().unwrap().parse().unwrap();
        let y: u128 = c["output"].as_str().unwrap().parse().unwrap();
        let by_target = c["by_target"].as_bool().unwrap();
        let gross = amount(&o, by_target, if by_target { y } else { x }).unwrap();
        assert_eq!(
            with_fee(gross, by_target, pair["fee_ppm"].as_u64().unwrap() as u32),
            Some(if by_target { x } else { y })
        );
        checked += 1;
    }
    assert_eq!(checked, 12);
}

#[test]
fn inventory_and_fixed_price_rounding_limit_both_sides() {
    let o = CarbonOrder {
        y: 10,
        z: 10,
        a: 0,
        b: ONE * 2 + ONE / 2,
    };
    assert_eq!(amount(&o, false, 2), Some(8));
    assert_eq!(amount(&o, false, 3), None);
    assert_eq!(amount(&o, true, 9), Some(3));
    assert_eq!(amount(&o, true, 11), None);
    assert_eq!(with_fee(100, true, 10000), Some(102));
    assert_eq!(with_fee(100, false, 10000), Some(99));
    assert_eq!(factor(U256::MAX, U256::MAX), Some(U256::MAX));
}
