//! Проверка математики Saddle по обменам без изменения состояния Ethereum.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t, rate_for_decimals};
use serde_json::Value;
use std::str::FromStr;

fn amount(value: &Value) -> U256 {
    U256::from_str(value.as_str().unwrap()).unwrap()
}

#[test]
fn registry_pause_and_permissionless_pool_swaps() {
    let doc: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/saddle_ethereum.json"
    )).unwrap();
    let pools = doc["pools"].as_object().unwrap();
    assert_eq!(pools.len(), 33);
    assert_eq!(pools.values().filter(|p| p["verified_pause"] == true).count(), 28);
    let active = doc["active"].as_array().unwrap();
    assert_eq!(active.len(), 1);
    let p = &pools[active[0].as_str().unwrap()];
    assert_eq!(p["clone_target"], doc["verified"]["PermissionlessSwap"]["address"]);
    let balances: Vec<_> = p["balances"].as_array().unwrap().iter().map(amount).collect();
    let rates: Vec<_> = p["decimals"].as_array().unwrap().iter()
        .map(|d| rate_for_decimals(d.as_u64().unwrap() as u8).unwrap()).collect();
    let template = Template { amp_precision: U256::from(100), max_rounds: 256, strict: true, pair_d: false };
    let swaps = p["swaps"].as_array().unwrap();
    assert_eq!(swaps.len(), 12);
    let mut coverage = [[0; 3]; 3];
    for case in swaps {
        let i = case["i"].as_u64().unwrap() as usize;
        let j = case["j"].as_u64().unwrap() as usize;
        let input = amount(&case["dx"]);
        let expected = amount(&case["out"]);
        let calls = &case["results"];
        assert_eq!(calls[4]["status"], "0x0");
        assert_eq!(calls[5]["status"], "0x1");
        let returned = |index: usize| amount(&calls[index]["returnData"]);
        assert_eq!(returned(1), expected);
        assert_eq!(returned(5), expected);
        assert_eq!(returned(2) - returned(6), input);
        assert_eq!(returned(7) - returned(3), expected);
        assert_eq!(exchange_out_t(i, j, input, &balances, &rates, amount(&p["a_precise"]),
                                 amount(&p["fee"]), &template), Some(expected));
        coverage[i][j] += 1;
    }
    assert_eq!(coverage, [[0, 2, 2], [2, 0, 2], [2, 2, 0]]);
}
