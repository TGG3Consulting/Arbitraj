use alloy_primitives::{Address, B256, U256, keccak256};
use arb::venues::bsc_dex::{
    math::{
        adaptive_fee::FeeConfig,
        fee_algebra_v1::{Oracle, Timepoint, WindowStart, fee_at},
        swap_algebra::{AlgebraState, NextTick, swap},
    },
    pools::ticks::{TickData, TickMap},
};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{collections::BTreeMap, fmt::Debug, str::FromStr};

fn number<T: FromStr>(v: &Value) -> T
where
    T::Err: Debug,
{
    v.as_str()
        .map_or_else(|| v.to_string(), str::to_owned)
        .parse()
        .unwrap()
}
fn point(v: &Value) -> Timepoint {
    Timepoint {
        initialized: v[0].as_bool().unwrap(),
        ts: number(&v[1]),
        tick_cum: number(&v[2]),
        vol: number(&v[4]),
        avg_tick: number(&v[5]),
        vpl: number(&v[6]),
    }
}

#[test]
fn synth_single_fee_oracle_and_raw_rows_match_registered_wallet_swaps() {
    let load = |raw: &[u8]| -> Value { serde_json::from_reader(GzDecoder::new(raw)).unwrap() };
    let d = load(include_bytes!(
        "data/first_run_dex_2026-11-26/base_algebra_synthswap-v3.json.gz"
    ));
    let p = load(include_bytes!(
        "data/first_run_dex_2026-11-26/synth_oracle_base.json.gz"
    ));
    assert_eq!(d["selector"], p["selector"]);
    let source = &p["oracle"];
    let all: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let f = &all
        .iter()
        .find(|row| row["id"] == "synthswap-v3" && row["chain_id"] == 8453)
        .unwrap();
    assert_eq!(
        f["venue_settings"]["dex.fusion_fee_horizon_s"],
        source["horizon"]
    );
    assert_eq!(
        f["venue_settings"]["dex.fusion_oracle_chunk"],
        source["chunk"]
    );
    let f = &f["settings"];
    for (i, key) in [(0, "eternal_farming"), (1, "farming_center")] {
        assert_eq!(f["dex.plugin_runtime_hashes"][i], p["roles"][key]["keccak"]);
    }
    let addr = |v: &Value| number::<Address>(v);
    let factory = addr(&d["roles"]["deployer"]["address"]);
    let mut salt = [0u8; 64];
    salt[12..32].copy_from_slice(addr(&d["tokens"][0]).as_slice());
    salt[44..64].copy_from_slice(addr(&d["tokens"][1]).as_slice());
    assert_eq!(
        factory.create2(
            keccak256(salt),
            number::<B256>(&f["dex.pool_code_hashes"][0])
        ),
        addr(&d["roles"]["pool"]["address"])
    );
    assert_eq!(
        addr(&p["roles"]["eternal_farming"]["address"]).create(1),
        addr(&p["roles"]["incentive"]["address"])
    );
    let h = source["headers"].as_array().unwrap();
    let stamp = |v: &Value| {
        u32::from_str_radix(
            v["timestamp"].as_str().unwrap().trim_start_matches("0x"),
            16,
        )
        .unwrap()
    };
    let mut deviations = Vec::new();
    for pair in h.windows(2) {
        assert_eq!(pair[0]["hash"], pair[1]["parentHash"]);
        deviations.push(stamp(&pair[1]) - stamp(&pair[0]));
    }
    assert!(deviations.contains(&0));
    assert_eq!(
        deviations.into_iter().max().unwrap(),
        number::<u32>(&f["dex.fusion_next_block_s"])
    );
    let spacing = number::<i32>(&d["tick_spacing"]);
    let bound = (887272 / spacing) * spacing;
    for t in [-bound, bound] {
        let word = number::<U256>(&source["boundary_words"][t.div_euclid(256).to_string()]);
        assert!(word.bit(t.rem_euclid(256) as usize));
    }
    let rates = d["reads"]["feeConfig()"]
        .as_str()
        .unwrap()
        .trim_start_matches("0x");
    let rate = |i: usize| u32::from_str_radix(&rates[i * 64..(i + 1) * 64], 16).unwrap();
    let config = FeeConfig {
        alpha1: rate(0) as u16,
        alpha2: rate(1) as u16,
        beta1: rate(2),
        beta2: rate(3),
        gamma1: rate(4) as u16,
        gamma2: rate(5) as u16,
        volume_beta: rate(6),
        volume_gamma: rate(7) as u16,
        base_fee: rate(8) as u16,
    };
    let ts = number::<u32>(&source["timestamp"]);
    let tp = &source["timepoints"];
    let gs = &d["global_state"];
    let starts = source["starts"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, v)| {
            (
                ts + i as u32,
                WindowStart {
                    tick_cum: number(&v[0]),
                    vol: number(&v[1]),
                    vpl: number(&v[2]),
                },
            )
        })
        .collect();
    let slot = U256::from_str_radix(
        source["slot"].as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    assert_eq!(
        slot & U256::from(u128::MAX),
        number::<U256>(&d["liquidity"])
    );
    let oracle = Oracle {
        cfg: config,
        index: number(&gs[3]),
        fee: number(&gs[2]),
        prev: point(&tp[0]),
        last: point(&tp[1]),
        next1: point(&tp[2]),
        next2: point(&tp[3]),
        zero: point(&tp[4]),
        vpl_in_block: (slot >> 128usize).to(),
        starts,
    };
    let tick = number::<i32>(&gs[1]);
    let fee = fee_at(&oracle, ts + number::<u32>(&source["block_step"]), tick).unwrap();
    let ticks = source["ticks"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                k.parse().unwrap(),
                TickData {
                    gross: number(&v[0]),
                    net: number(&v[1]),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let map = TickMap {
        spacing: 1,
        words: (number(&source["words"][0]), number(&source["words"][1])),
        ticks: ticks.into(),
    };
    let state = AlgebraState {
        sqrt_price: number(&gs[0]),
        tick,
        liquidity: number(&d["liquidity"]),
        fee,
        ticks: &map,
        next: NextTick::Row,
    };
    for c in d["cases"].as_array().unwrap() {
        let after = c["results"]["after_global"]["returnData"].as_str().unwrap();
        assert_eq!(fee, u32::from_str_radix(&after[130..194], 16).unwrap());
        let input = number(&c["amount"]);
        let output = swap(&state, c["token_in"] == d["tokens"][0], true, input).unwrap();
        assert!(output.complete);
        assert_eq!(
            (output.amount_in, output.amount_out),
            (input, number(&c["bought"]))
        );
    }
}
