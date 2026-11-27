//! Комиссия, оставшийся вход и выплата E3 проверяются на всех четырёх реальных контрактах Base.
use alloy_primitives::{Address, Bytes, U256};
use arb::venues::bsc_dex::{
    math::lb_swap,
    pools::{bins::BinMap, load_lb},
};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::BTreeMap;

fn n(v: &Value) -> u128 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn exact_quotes_across_every_occupied_bin_and_the_cash_limit() {
    let d: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/e3_base_bins.json.gz")[..],
    ))
    .unwrap();
    let stamp = d["stamp"].as_u64().unwrap();
    let mut total = 0;
    for p in d["pools"].as_array().unwrap() {
        let x = p["x"].as_str().unwrap().parse::<Address>().unwrap();
        let y = p["y"].as_str().unwrap().parse::<Address>().unwrap();
        let step = p["step"].as_u64().unwrap() as u32;
        let words = |v: Vec<u64>| {
            Bytes::from(
                v.into_iter()
                    .flat_map(|w| U256::from(w).to_be_bytes::<32>())
                    .collect::<Vec<_>>(),
            )
        };
        let fields = |k: &str| {
            p[k].as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect()
        };
        let state = load_lb::decode(
            &[
                words(vec![p["active"].as_u64().unwrap()]),
                words(fields("statics")),
                words(fields("variable")),
            ],
            x < y,
            step,
            stamp,
        )
        .unwrap();
        let reserves = p["bins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| (b[0].as_u64().unwrap() as u32, (n(&b[1]), n(&b[2]))))
            .collect::<BTreeMap<_, _>>();
        let bins = BinMap {
            step,
            reserves: reserves.into(),
            range: (0, 0xff_ffff),
        };
        for q in p["quotes"].as_array().unwrap() {
            let r = lb_swap::swap_out(
                &state,
                &bins,
                stamp,
                q["for_y"].as_bool().unwrap(),
                n(&q["qty"]),
            )
            .unwrap();
            let actual = &q["result"];
            assert_eq!(
                [r.in_left, r.out, r.fee],
                [n(&actual[0]), n(&actual[1]), n(&actual[2])],
                "E3 {} {}",
                p["pool"],
                q
            );
            total += 1;
        }
    }
    assert_eq!(total, 52);
}
