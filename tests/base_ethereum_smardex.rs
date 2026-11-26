//! Ответы SmarDexRouter проверяют формулу пары на обеих сетях, включая расхождение approxEq.
use alloy_primitives::U256;
use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::{smardex::Params, smardex_swap};
use arb::venues::bsc_dex::pools::load_smardex;
use serde_json::Value;
use std::str::FromStr;

fn number(v: &Value) -> U256 {
    U256::from_str(v.as_str().unwrap()).unwrap()
}

#[test]
fn deployed_routers_match_their_factory_math() {
    for src in [
        include_str!("data/first_run_dex_2026-11-26/smardex_base.json"),
        include_str!("data/first_run_dex_2026-11-26/smardex_ethereum.json"),
    ] {
        let doc: Value = serde_json::from_str(src).unwrap();
        let exact = doc["explicit_equality"].as_bool().unwrap();
        assert_eq!(exact, doc["chain_id"] == 1);
        let now = doc["timestamp"].as_u64().unwrap();
        for row in doc["pools"].as_array().unwrap() {
            let pair = |i: &str| (number(&row[i][0]), number(&row[i][1]));
            let raw = load_smardex::Raw {
                reserves: pair("reserves"),
                fictive: pair("fictive"),
                price: pair("price"),
                price_ts: number(&row["price"][2]),
                fees: pair("fee"),
                block_ts: U256::from(now),
            };
            let PoolState::Fictive(state) = load_smardex::state(&raw).unwrap() else {
                panic!("обработчик SmarDex")
            };
            assert_eq!(row["quotes"].as_array().unwrap().len(), 4);
            for q in row["quotes"].as_array().unwrap() {
                let side = smardex_swap::side(&state, q["i"] == 0);
                let got = smardex_swap::swap_out_version(
                    &side,
                    state.price_average_ts,
                    now,
                    number(&q["amount_in"]),
                    exact,
                );
                assert_eq!(got, Some(number(&q["amount_out"])), "{}", row["pool"]);
            }
        }
        let item = &doc["equal_price_example"];
        let p = |i: usize| U256::from(item["params"][i].as_u64().unwrap());
        let params = Params {
            reserve_in: p(1),
            reserve_out: p(2),
            fictive_in: p(3),
            fictive_out: p(4),
            price_in: p(5),
            price_out: p(6),
            fees_lp: p(7),
            fees_pool: p(8),
        };
        let expected = Some(number(&item["result"][0]));
        assert_eq!(
            smardex_swap::swap_out_version(&params, now, now, p(0), exact),
            expected
        );
        assert_ne!(
            smardex_swap::swap_out_version(&params, now, now, p(0), !exact),
            expected
        );
    }
}
