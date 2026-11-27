use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

fn integer(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn from_row(d: &Value, row: &Value) -> Market {
    let tokens = [addr(&row["tokens"][0]), addr(&row["tokens"][1])];
    let oracle = &d["oracles"][row["oracle"].as_str().unwrap()];
    Market {
        tokens,
        atf: addr(&d["fee_token"]),
        tier: row["fee"].as_u64().unwrap() as u16,
        reserves: std::array::from_fn(|i| integer(&row["reserves"][i])),
        balances: std::array::from_fn(|i| integer(&row["balances"][i])),
        dividends: integer(&row["escrow"]),
        lp_supply: integer(&row["supply"]),
        oracle: (!oracle.is_null()).then(|| {
            (
                tokens
                    .iter()
                    .position(|t| *t == addr(&oracle["asset"]))
                    .unwrap(),
                integer(&oracle["rate_q112"]),
            )
        }),
    }
}

#[test]
fn fully_funded_fill_prices_refilling_the_fee_token_in_the_input_asset() {
    let registry = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_ethereum_registry.json.gz"
    );
    let protected = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_protected_ethereum.json.gz"
    );
    let d: Value = serde_json::from_reader(GzDecoder::new(registry.as_slice())).unwrap();
    let controls: Value = serde_json::from_reader(GzDecoder::new(protected.as_slice())).unwrap();
    let rows = d["pairs"].as_array().unwrap();
    let weth = alloy_primitives::address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
    let fee_market = from_row(
        &d,
        rows.iter()
            .find(|r| {
                addr(&r["tokens"][0]) == addr(&d["fee_token"])
                    && addr(&r["tokens"][1]) == weth
                    && r["fee"] == 100
            })
            .unwrap(),
    );
    let cases = controls["cases"].as_array().unwrap();
    for trial in cases {
        let pool = from_row(
            &d,
            rows.iter().find(|r| r["pair"] == trial["pool"]).unwrap(),
        );
        let f = pool
            .settle(weth, integer(&trial["primary_input"]), false)
            .unwrap();
        let restored = fee_market.acquire_fee(weth, f.separate_atf).unwrap();
        assert_eq!(restored.output, f.separate_atf);
        assert_eq!(restored.debit, integer(&trial["funding_cost"]));
        assert_eq!(f.output, integer(&trial["net"][1]));
        let held = trial["mode"] == "held_fee";
        assert_eq!(
            f.debit + if held { U256::ZERO } else { restored.debit },
            integer(&trial["net"][0])
        );
        assert_eq!(
            if held { f.separate_atf } else { U256::ZERO },
            integer(&trial["net"][2])
        );
    }
    assert_eq!(cases.len(), 16);
}

#[test]
fn both_router_methods_match_owner_receipts_in_both_directions() {
    let fixture = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_wallets_ethereum.json.gz"
    );
    let data: Value = serde_json::from_reader(GzDecoder::new(fixture.as_slice())).unwrap();
    let d = &data["directory"];
    let mut yes = 0;
    let mut no = 0;
    for trace in data["outcomes"].as_array().unwrap() {
        let row = d["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["pair"] == trace["pool"])
            .unwrap();
        let market = from_row(d, row);
        let tokens = market.tokens;
        for trial in trace["examples"].as_array().unwrap() {
            let mut state = market.clone();
            for leg in [trial.get("forward"), trial.get("reverse")]
                .into_iter()
                .flatten()
            {
                if leg.get("source").is_none() {
                    assert_eq!(leg["rejected"], "input below router rounding");
                    continue;
                }
                let source = addr(&leg["source"]);
                let supporting = leg["supporting"].as_bool().unwrap();
                let fill = state.settle(source, integer(&leg["budget"]), supporting);
                if leg.get("output").is_none() {
                    assert!(fill.is_none(), "{leg}");
                    no += 1;
                } else {
                    let f = fill
                        .unwrap_or_else(|| panic!("missed {} {}", row["pair"], leg["transaction"]));
                    assert_eq!(f.output, integer(&leg["output"]));
                    assert_eq!(f.parameter, integer(&leg["parameter"]));
                    assert_eq!(f.fee_limit, integer(&leg["fee_ceiling"]));
                    let delta = |token: Address| -> U256 {
                        leg["measured"][format!("{token:#x}")]
                            .as_str()
                            .unwrap()
                            .trim_start_matches('-')
                            .parse()
                            .unwrap()
                    };
                    assert_eq!(f.debit, delta(source));
                    if !tokens.contains(&state.atf) {
                        assert_eq!(f.separate_atf, delta(state.atf));
                    }
                    state = f.next;
                    yes += 1;
                }
            }
        }
    }
    assert_eq!((yes, no), (98, 24));
}
