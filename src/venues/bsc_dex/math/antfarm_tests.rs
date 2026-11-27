use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

fn integer(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
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
        let tokens = [addr(&row["tokens"][0]), addr(&row["tokens"][1])];
        let oracle = &d["oracles"][row["oracle"].as_str().unwrap()];
        let market = Market {
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
        };
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
