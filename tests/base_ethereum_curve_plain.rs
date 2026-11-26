//! Сопоставляет формулы Curve Plain и PancakeSwap StableSwap с закреплёнными get_dy двух сетей.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t, fee_denominator};
use serde_json::Value;
use std::str::FromStr;

fn integer(value: &Value) -> U256 {
    U256::from_str(value.as_str().unwrap()).unwrap()
}

#[test]
fn plain_factory_markets_match_dy() {
    let exchanges: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/pancake_stable_exchange_ethereum.json"
    )).unwrap();
    for (src, plain) in [
        (
            include_str!("data/first_run_dex_2026-11-26/curve_plain_base.json"),
            true,
        ),
        (
            include_str!("data/first_run_dex_2026-11-26/curve_plain_ethereum.json"),
            true,
        ),
        (
            include_str!("data/first_run_dex_2026-11-26/pancake_stable_ethereum.json"),
            false,
        ),
    ] {
        let doc: Value = serde_json::from_str(src).unwrap();
        let pools = doc[if plain { "pools" } else { "markets" }]
            .as_array()
            .unwrap();
        assert_eq!(pools.len(), if plain { 4 } else { 2 });
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
                amp_precision: U256::from(if plain { 100u8 } else { 1u8 }),
                max_rounds: 255,
                strict: plain,
                pair_d: plain && balances.len() == 2,
            };
            for q in p["quotes"].as_array().unwrap() {
                let quote_with_fee = |fee| exchange_out_t(
                    q["i"].as_u64().unwrap() as usize,
                    q["j"].as_u64().unwrap() as usize,
                    integer(&q["dx"]),
                    &balances,
                    &rates,
                    integer(&p["a"]),
                    fee,
                    &t,
                );
                let fee = integer(&p["fee"]);
                let expected = if plain {
                    integer(&q["dy"])
                } else {
                    // Pancake get_dy: перевод gross в единицы токена до комиссии.
                    let gross = quote_with_fee(U256::ZERO).unwrap();
                    assert_eq!(gross - gross * fee / fee_denominator(), integer(&q["dy"]));
                    // exchange: комиссия до перевода в единицы токена. Источник — дельта balanceOf.
                    let case = exchanges["cases"].as_array().unwrap().iter().find(|c| {
                        c["pool"] == p["pool"] && c["i"] == q["i"] && c["dx"] == q["dx"]
                    }).unwrap();
                    assert_eq!(case["calls"][4]["status"], "0x0");
                    assert_eq!(case["calls"][5]["status"], "0x1");
                    let raw = |i: usize| integer(&case["calls"][i]["returnData"]);
                    assert_eq!(raw(2) - raw(6), integer(&q["dx"]));
                    assert_eq!(raw(7) - raw(3), integer(&case["exchange_out"]));
                    raw(7) - raw(3)
                };
                assert_eq!(quote_with_fee(fee), Some(expected), "{}", p["pool"]);
            }
        }
    }
}
