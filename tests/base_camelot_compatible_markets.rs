//! Сверяет фабрики Base, направленные комиссии и чистый выход штатного V5 по архивному состоянию каждой ноги.
use alloy_primitives::{Address, U256};
use arb::venues::bsc_dex::{
    math::camelot_v2::{Pair, swap_out},
    pools::verify::v2_create2,
};
use serde_json::Value;

fn uint(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn registered_pairs_and_exact_wallet_floors_match_both_curves_and_directions() {
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    for (id, fixture, total, stable, trials) in [
        (
            "citadelswap",
            include_str!("data/first_run_dex_2026-11-26/citadelswap.json"),
            6,
            2,
            6,
        ),
        (
            "kim-v2-base",
            include_str!("data/first_run_dex_2026-11-26/kim_v2_base.json"),
            1,
            0,
            2,
        ),
    ] {
        let data: Value = serde_json::from_str(fixture).unwrap();
        let directory = &data["directory"];
        let execution = &data["execution"];
        let config = &profiles
            .iter()
            .find(|p| p["id"] == id && p["chain_id"] == 8453)
            .unwrap()["settings"];
        assert_eq!(config["dex.kind"], "camelot_v2");
        assert_eq!(config["dex.factory"], directory["factory"]);
        assert_eq!(directory["selector"], execution["selector"]);
        let factory = config["dex.factory"].as_str().unwrap().parse().unwrap();
        let init = directory["pair_init_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let pools = directory["markets"].as_array().unwrap();
        assert_eq!(pools.len(), total);
        assert_eq!(pools.iter().filter(|p| p["stable"] == true).count(), stable);
        for p in pools {
            let addr = |i: usize| p["tokens"][i].as_str().unwrap().parse().unwrap();
            assert_eq!(
                v2_create2(factory, addr(0), addr(1), init),
                p["pool"].as_str().unwrap().parse::<Address>().unwrap()
            );
        }
        let cases = execution["cases"].as_array().unwrap();
        assert_eq!(cases.len(), trials);
        for row in cases {
            let m = &row["market"];
            assert!(pools.contains(m));
            let legs = &row["result"];
            assert_eq!(legs["buy"]["exact"]["received"], legs["sell"]["amount"]);
            for side in ["buy", "sell"] {
                let leg = &legs[side];
                let exact = &leg["exact"];
                let r = &exact["reserves"];
                let units = &exact["units"];
                let pair = Pair {
                    reserve0: uint(&r[0]),
                    reserve1: uint(&r[1]),
                    fee0: uint(&r[2]),
                    fee1: uint(&r[3]),
                    stable: m["stable"].as_bool().unwrap(),
                    pm0: uint(&units[0]),
                    pm1: uint(&units[1]),
                };
                let zfo = leg["input"] == m["tokens"][0];
                let amount = uint(&leg["amount"]);
                assert_eq!(exact["status"], "0x1");
                assert_eq!(uint(&exact["spent"]), amount);
                assert_eq!(swap_out(&pair, amount, zfo), Some(uint(&exact["received"])));
                assert_eq!(exact["recipient_quote"], exact["received"]);
                let reject = &leg["plus_one"];
                assert_eq!(reject["status"], "0x0");
                assert_eq!(
                    (uint(&reject["spent"]), uint(&reject["received"])),
                    (U256::ZERO, U256::ZERO)
                );
            }
        }
    }
}
