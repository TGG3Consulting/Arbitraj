//! Сверка PMM и пользовательских комиссий DODO с двумя объёмами querySellBase / querySellQuote
//! каждой из трёх фабрик обеих сетей на закреплённом блоке.
use std::str::FromStr;

use alloy_primitives::U256;
use arb::venues::bsc_dex::math::swap_dodo::{PmmState, sell_base_token, sell_quote_token};
use arb::venues::bsc_dex::pools::load_dodo::{net_after_fees, rstate};
use serde_json::Value;

fn num(v: &Value) -> U256 {
    U256::from_str(v.as_str().unwrap()).unwrap()
}

#[test]
fn net_pmm_quotes_match_the_registered_pools() {
    for data in [
        include_str!("data/first_run_dex_2026-11-26/dodo_base.json"),
        include_str!("data/first_run_dex_2026-11-26/dodo_ethereum.json"),
    ] {
        let doc: Value = serde_json::from_str(data).unwrap();
        for kind in ["DVM", "DPP", "DSP"] {
            let mut markets = 0;
            for row in doc["families"][kind]["pools"].as_array().unwrap() {
                if row["status"] != "quoted" {
                    continue;
                }
                let s = &row["state"];
                let state = PmmState {
                    i: num(&s[0]),
                    k: num(&s[1]),
                    b: num(&s[2]),
                    q: num(&s[3]),
                    b0: num(&s[4]),
                    q0: num(&s[5]),
                    r: rstate(num(&s[6]).to::<u8>()).unwrap(),
                };
                let steps = row["quotes"].as_array().unwrap();
                assert_eq!(steps.len(), 4);
                let mut count = [0; 2];
                for q in steps {
                    let dx = num(&q["amount_in"]);
                    let (gross, _) = match q["sell"].as_str().unwrap() {
                        "querySellBase" => {
                            count[0] += 1;
                            sell_base_token(&state, dx)
                        }
                        "querySellQuote" => {
                            count[1] += 1;
                            sell_quote_token(&state, dx)
                        }
                        method => panic!("неизвестный метод {method}"),
                    }
                    .unwrap();
                    let net =
                        net_after_fees(gross, num(&row["fees"][0]), num(&row["fees"][1])).unwrap();
                    assert_eq!(net, num(&q["amount_out"]), "{kind} {} {dx}", row["pool"]);
                }
                assert_eq!(count, [2, 2]);
                markets += 1;
            }
            assert!(markets > 0, "{kind}, chain {}", doc["chain_id"]);
        }
    }
}
