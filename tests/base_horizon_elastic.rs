use alloy_primitives::U256;
use arb::venues::bsc_dex::{
    math::swap_elastic::{ElasticState, swap},
    pools::ticks::{TickData, TickMap},
};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::BTreeMap;

fn decimal(v: &Value) -> U256 {
    v.as_str()
        .map_or_else(|| v.to_string(), str::to_owned)
        .parse()
        .unwrap()
}
fn word(results: &Value, key: &str) -> U256 {
    U256::from_str_radix(
        results[key]["returnData"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap()
}

#[test]
fn horizon_full_input_math_router_and_executor_share_net_payout() {
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/horizondex_base.json.gz");
    let d: Value = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
    let cfg: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let cfg = &cfg
        .iter()
        .find(|x| x["id"] == "horizondex-base" && x["chain_id"] == 8453)
        .unwrap()["settings"];
    assert_eq!(cfg["dex.kind"], "kyber_elastic_v5");
    for name in ["factory", "router", "quoter", "tick_reader"] {
        assert_eq!(cfg[format!("dex.{name}")], d["roles"][name]["address"]);
        assert_eq!(
            cfg[format!("dex.{name}_runtime_hash")],
            d["roles"][name]["runtime_hash"]
        );
    }
    assert_eq!(cfg["dex.elastic_ticks_chunk"], d["tick_reader_chunk"]);
    let ticks = d["ticks"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(t, v)| {
            let gross = v[0].to_string().parse().unwrap();
            (gross > 0).then(|| {
                (
                    t.parse().unwrap(),
                    TickData {
                        gross,
                        net: v[1].to_string().parse().unwrap(),
                    },
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let map = TickMap {
        spacing: d["spacing"].as_i64().unwrap() as i32,
        ticks: ticks.into(),
        words: (i16::MIN, i16::MAX),
    };
    let state = ElasticState {
        sqrt_p: decimal(&d["state"][0]),
        current_tick: d["state"][1].as_i64().unwrap() as i32,
        base_l: d["liquidity"][0].to_string().parse().unwrap(),
        reinvest_l: d["liquidity"][1].to_string().parse().unwrap(),
        fee_units: d["fee"].as_u64().unwrap() as u32,
        ticks: &map,
        v1: false,
    };
    for case in d["cases"].as_array().unwrap() {
        let amount = decimal(&case["buy"]["amount"]);
        let local = swap(&state, true, true, amount).unwrap();
        assert!(local.complete);
        assert_eq!(
            (local.amount_in, local.amount_out),
            (amount, decimal(&case["buy"]["received"]))
        );
        let parent = &case["buy"]["results"];
        let field = |key: &str, i: usize| {
            let raw = parent[key]["returnData"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x");
            U256::from_str_radix(&raw[i * 64..i * 64 + 64], 16).unwrap()
        };
        let after = ElasticState {
            sqrt_p: field("buypool_state", 0),
            current_tick: i32::try_from(alloy_primitives::I256::from_raw(field(
                "buypool_state",
                1,
            )))
            .unwrap(),
            base_l: field("buyliquidity", 0).to(),
            reinvest_l: field("buyliquidity", 1).to(),
            fee_units: state.fee_units,
            ticks: &map,
            v1: false,
        };
        let second = swap(&after, false, true, local.amount_out).unwrap();
        assert!(second.complete);
        assert_eq!(
            (second.amount_in, second.amount_out),
            (local.amount_out, decimal(&case["sell"]["received"]))
        );
        for side in ["buy", "sell"] {
            let used = decimal(&case[side]["amount"]);
            let out = decimal(&case[side]["received"]);
            for (is_executor, leg) in [(false, &case[side]), (true, &case["v5"][side])] {
                let r = &leg["results"];
                let n = |k: &str| word(r, &format!("{side}{k}"));
                assert_eq!(n("start_in") - n("end_in"), used);
                assert_eq!(n("end_out") - n("start_out"), out);
                assert_eq!(n("exact"), out);
                for k in ["above", "expired"] {
                    assert_eq!(r[format!("{side}{k}")]["status"], "0x0");
                }
                if is_executor {
                    assert_eq!((n("left_in"), n("left_out")), (U256::ZERO, U256::ZERO));
                }
            }
        }
    }
}
