use alloy_primitives::{Address, B256, U256};
use arb::{
    core::types::{EkuboData, EkuboTick},
    venues::bsc_dex::{
        math::ekubo_swap,
        pools::{abi_ekubo, ekubo_v2, load_ekubo},
    },
};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::HashSet;
fn word(x: &Value) -> B256 {
    x.as_str().unwrap().parse().unwrap()
}
fn address(x: &Value) -> Address {
    x.as_str().unwrap().parse().unwrap()
}
fn integer(x: &Value) -> i128 {
    x.as_str().unwrap().parse().unwrap()
}

#[test]
fn ethereum_v2_activity_registry_and_exact_input_match_at_the_saved_hash() {
    let src = include_bytes!("data/first_run_dex_2026-11-26/ekubo_v2_ethereum_activity.json.gz");
    let v: Value = serde_json::from_reader(GzDecoder::new(src.as_slice())).unwrap();
    let rows = v["swaps"].as_array().unwrap();
    let mut txs = HashSet::new();
    for row in rows {
        assert!(txs.insert(row["tx"].as_str().unwrap()));
        let bytes = alloy_primitives::hex::decode(row["wire"].as_str().unwrap()).unwrap();
        let (id, balance, _) = abi_ekubo::decode_swap_log(&bytes).unwrap();
        assert_eq!(id, word(&row["id"]));
        assert_eq!(
            abi_ekubo::decode_balance(&balance),
            (integer(&row["delta0"]), integer(&row["delta1"]))
        );
    }
    assert_eq!((v["logs"].as_u64(), rows.len()), (Some(126), 123));
    let mut count = (0, 0);
    for m in v["markets"].as_array().unwrap() {
        let k = &m["key"];
        let config = word(&k["config"]);
        let id = abi_ekubo::pool_id(address(&k["token0"]), address(&k["token1"]), config);
        assert_eq!(id, word(&m["id"]));
        assert_eq!(ekubo_v2::state_slot(id), word(&m["slot"]));
        let (sqrt_ratio, tick, liquidity) = ekubo_v2::decode_state(&word(&m["state"]));
        let window = &m["tick_window"];
        let words = (
            window["range"][0].as_u64().unwrap(),
            window["range"][1].as_u64().unwrap(),
        );
        let bitmap: Vec<_> = window["masks"]
            .as_array()
            .unwrap()
            .iter()
            .zip(words.0..=words.1)
            .map(|(w, i)| (i, U256::from_be_bytes(word(w).0)))
            .collect();
        let ticks: Vec<_> = window["ticks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                let (net, delta) = abi_ekubo::decode_tick(&word(&v[1]));
                EkuboTick {
                    tick: v[0].as_i64().unwrap() as i32,
                    net,
                    delta,
                }
            })
            .collect();
        assert_eq!(
            load_ekubo::set_ticks(&bitmap, k["tick_spacing"].as_u64().unwrap() as u32),
            ticks.iter().map(|v| v.tick).collect::<Vec<_>>()
        );
        let state = EkuboData {
            config: ekubo_v2::math_config(config).unwrap(),
            sqrt_ratio,
            tick,
            liquidity,
            words,
            bitmap,
            ticks,
            native: None,
            block_ts: v["window"][1]["time"].as_u64().unwrap(),
            mev: None,
            twamm: None,
        };
        for q in m["quotes"].as_array().unwrap() {
            let side = q["direction"] == 1;
            let dx = integer(&q["amount"]);
            let order = ekubo_v2::OrderKey {
                token0: address(&k["token0"]),
                token1: address(&k["token1"]),
                config,
                side,
                amount: dx,
            };
            let (d0, d1) = (integer(&q["delta0"]), integer(&q["delta1"]));
            let (used, out) = if side { (d1, -d0) } else { (d0, -d1) };
            let mut answer = Vec::new();
            for delta in [d0, d1] {
                answer.extend([if delta < 0 { 0xff } else { 0 }; 16]);
                answer.extend(delta.to_be_bytes());
            }
            assert_eq!(
                order.payout(&answer),
                (used == dx).then_some(U256::from(out.unsigned_abs()))
            );
            let local = ekubo_swap::exact_in(&state, side, dx.unsigned_abs());
            if q["full"] == true {
                let sample = local.unwrap();
                assert!(sample.complete);
                assert_eq!(
                    (sample.amount_in, sample.amount_out),
                    (dx.unsigned_abs(), out.unsigned_abs()),
                    "{id}"
                );
                count.0 += 1;
            } else {
                assert!(local.is_none_or(|s| !s.complete || s.amount_in < dx.unsigned_abs()));
                count.1 += 1;
            }
        }
    }
    assert_eq!(count, (18, 2));
}
