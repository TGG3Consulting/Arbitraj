//! Сопоставление Curve IBT/PT со всеми публичными get_dy Spectra при реальных rates, fee и остатках.
mod curve_crypto_common;
mod multi_common;
#[path = "base_spectra_math/routing.rs"]
mod routing;

use alloy_primitives::{Address, U256};
use arb::venues::bsc_dex::{
    math::{crypto_swap, stableswap_ng},
    pools::load_crypto_v1,
};
use serde_json::Value;

fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

fn stable(r: &Value, i: usize, input: U256) -> Option<U256> {
    let calls = &r["calls"];
    let rates: Vec<_> = calls["stored_rates()"]
        .as_array()?
        .iter()
        .map(num)
        .collect();
    let balances: Vec<_> = r["balances"].as_array()?.iter().map(num).collect();
    stableswap_ng::exchange_received(
        i,
        1 - i,
        input,
        &balances,
        &rates,
        num(&calls["A_precise()"]),
        num(&calls["fee()"]),
        num(&calls["offpeg_fee_multiplier()"]),
    )
    .map(|x| x.0)
}

#[test]
fn spectra_ibts_and_pts_match_all_canonical_steps() {
    let raw = include_bytes!("data/first_run_dex_2026-11-26/spectra_base_math.json.gz");
    let book: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
    let mut totals = [0usize; 2];
    for r in book["markets"].as_array().unwrap() {
        let pool: Address = r["pool"].as_str().unwrap().parse().unwrap();
        let crypto = (r["kind"] == "legacy").then(|| {
            let got = curve_crypto_common::answers(r, &load_crypto_v1::calls_with(pool, 2, false));
            let decimals = r["decimals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let snapshot = load_crypto_v1::Raw {
                pool,
                n: 2,
                got,
                now: book["timestamp"].as_u64().unwrap(),
                decimals,
                killed: false,
            };
            load_crypto_v1::decode(&snapshot).ok().map(|(_, s)| s)
        });
        for q in r["quotes"].as_array().unwrap() {
            let i = q["i"].as_u64().unwrap() as usize;
            let dx = num(&q["dx"]);
            let actual = match &crypto {
                Some(data) => data
                    .as_ref()
                    .and_then(|s| crypto_swap::exchange_out(s, i, 1 - i, dx)),
                None => stable(r, i, dx),
            };
            let expected = q["dy"].as_str().map(|s| s.parse::<U256>().unwrap());
            assert_eq!(actual, expected, "{pool}, i={i}, input={dx}");
            totals[usize::from(expected.is_some())] += 1;
        }
    }
    assert_eq!(totals, [88, 216]);
}
