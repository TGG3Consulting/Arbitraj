//! Сопоставляет локальную Infinity CL с выплатой кошельку и состоянием Swap обеих ног Omni Flux.
use alloy_primitives::U256;
use arb::venues::bsc_dex::{
    math::swap_cl::{ClState, Version, swap_final},
    pools::ticks::{TickData, TickMap},
    quote::pool_quote::swap_fee,
};
use serde_json::Value;
use std::{collections::BTreeMap, str::FromStr};

fn num<T: FromStr>(v: &Value) -> T
where
    T::Err: std::fmt::Debug,
{
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn omni_cl_wallet_and_next_state_match_across_the_entire_traversed_bitmap() {
    let raw = include_bytes!("data/first_run_dex_2026-11-26/omni_flux_cl_math.json.gz");
    let record: Value = serde_json::from_reader(flate2::read::GzDecoder::new(&raw[..])).unwrap();
    let mut count = 0;
    for m in record["markets"].as_array().unwrap() {
        let ticks: BTreeMap<_, _> = m["ticks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r[0].as_i64().unwrap() as i32,
                    TickData {
                        gross: num(&r[1]),
                        net: num(&r[2]),
                    },
                )
            })
            .collect();
        let map = TickMap {
            spacing: m["spacing"].as_i64().unwrap() as i32,
            ticks: ticks.into(),
            words: (
                m["words"][0].as_i64().unwrap() as i16,
                m["words"][1].as_i64().unwrap() as i16,
            ),
        };
        for cycle in m["replays"].as_array().unwrap() {
            let slot = &m["slot0"];
            let (mut sqrt, mut tick, mut liquidity) =
                (num(&slot[0]), num(&slot[1]), num(&m["liquidity"]));
            for leg in cycle["legs"].as_array().unwrap() {
                let forward = leg["forward"].as_bool().unwrap();
                let fee = swap_fee(num(&slot[2]), num(&slot[3]), forward);
                assert_eq!(u64::from(fee), leg["fee"].as_u64().unwrap());
                let state = ClState {
                    sqrt_price: sqrt,
                    tick,
                    liquidity,
                    fee,
                    ticks: &map,
                    version: Version::V4,
                };
                let paid = num(&leg["paid"]);
                let (fill, after) = swap_final(&state, forward, true, paid).unwrap();
                assert!(fill.complete);
                assert_eq!(fill.amount_in, paid);
                assert_eq!(fill.amount_out, num::<U256>(&leg["net"]), "{}", m["id"]);
                assert_eq!(after.sqrt_price, num::<U256>(&leg["sqrt"]));
                assert_eq!(after.tick, leg["tick"].as_i64().unwrap() as i32);
                assert_eq!(after.liquidity, num::<u128>(&leg["liquidity"]));
                (sqrt, tick, liquidity) = (after.sqrt_price, after.tick, after.liquidity);
                count += 1;
            }
        }
    }
    assert_eq!(count, 44);
}

#[test]
fn bin_math_matches_actual_debit_and_payout_in_the_preceding_wallet_state() {
    use arb::venues::bsc_dex::{math::bin_swap, pools::bins::BinMap};
    let raw = include_bytes!("data/first_run_dex_2026-11-26/omni_flux_bin_wallets.json.gz");
    let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(&raw[..])).unwrap();
    let mut by_mode = BTreeMap::new();
    let mut one_way = false;
    let mut skipped_positive_reserves = false;
    for m in archive["markets"].as_array().unwrap() {
        let params: U256 = num(&m["key"][5]);
        let step = ((params >> 16u32) & U256::from(0xffffu32)).to::<u32>();
        for c in m["cases"].as_array().unwrap() {
            if c["buy"]["stage"] == "accepted" && c["sell"]["stage"] == "quote" {
                one_way = true;
            }
            for leg in [&c["buy"], &c["sell"]]
                .into_iter()
                .filter(|v| v["stage"] == "accepted")
            {
                let s = &leg["bin_before"];
                let active = s["active"].as_u64().unwrap() as u32;
                let last = s["last"].as_u64().unwrap() as u32;
                let forward = s["forward"].as_bool().unwrap();
                let fee = swap_fee(
                    s["protocol"].as_u64().unwrap() as u32,
                    s["lp"].as_u64().unwrap() as u32,
                    forward,
                );
                assert_eq!(u64::from(fee), s["event_fee"].as_u64().unwrap());
                let bins: BTreeMap<_, _> = s["bins"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| (r[0].as_u64().unwrap() as u32, (num(&r[1]), num(&r[2]))))
                    .collect();
                let links = s["successors"].as_array().unwrap();
                let mut next = active;
                for edge in links {
                    assert_eq!(edge[0].as_u64(), Some(u64::from(next)));
                    next = edge[1].as_u64().unwrap() as u32;
                    assert!(bins.contains_key(&next));
                }
                assert_eq!(next, last);
                for row in s["skipped_reserves"].as_array().unwrap() {
                    let id = row[0].as_u64().unwrap() as u32;
                    assert!(!bins.contains_key(&id));
                    if num::<U256>(&row[1]) > U256::ZERO || num::<U256>(&row[2]) > U256::ZERO {
                        skipped_positive_reserves = true;
                    }
                }
                let map = BinMap {
                    step,
                    reserves: bins.into(),
                    range: (active.min(last), active.max(last)),
                };
                let f =
                    bin_swap::swap(&map, active, fee, forward, true, num(&leg["input"])).unwrap();
                assert!(f.complete);
                assert_eq!(f.amount_in, num::<U256>(&leg["paid"]));
                assert_eq!(f.amount_out, num::<U256>(&leg["net"]), "{}", m["pool"]);
                assert_eq!(leg["quoted"], leg["net"]);
                *by_mode.entry(c["mode"].as_str().unwrap()).or_insert(0u32) += 1;
            }
        }
    }
    assert!(by_mode["router"] >= 21 && by_mode["v5"] >= 15);
    assert!(one_way);
    assert!(skipped_positive_reserves);
}
