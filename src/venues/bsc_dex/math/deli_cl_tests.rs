//! Сверка двух валют, обеих сторон, округления и нижней границы входа по выплатам Deli CL.
use super::deli_cl::quote;
use super::swap_cl::{ClState, Version};
use crate::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use alloy_primitives::U256;
use flate2::read::GzDecoder;
use serde_json::Value;

fn value(raw: &Value) -> U256 {
    raw.as_str().unwrap().parse().unwrap()
}
fn words(result: &Value) -> Vec<U256> {
    let raw = result.as_str().unwrap().trim_start_matches("0x");
    alloy_primitives::hex::decode(raw)
        .unwrap()
        .chunks_exact(32)
        .map(U256::from_be_slice)
        .collect()
}

#[test]
fn both_wallets_match_the_fee_and_initialized_ticks() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/deli_base_cl_wallets.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    let checked: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/deli_base_cl_ticks.json"
    ))
    .unwrap();
    assert_eq!(archive["selector"], checked["selector"]);
    let mut count = 0;
    for m in archive["markets"].as_array().unwrap() {
        let row = checked["markets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == m["pool_id"])
            .unwrap();
        let ticks = TickMap {
            spacing: row["spacing"].as_i64().unwrap() as i32,
            words: (
                row["words"][0].as_i64().unwrap() as i16,
                row["words"][1].as_i64().unwrap() as i16,
            ),
            ticks: row["ticks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    (
                        v[0].as_i64().unwrap() as i32,
                        TickData {
                            gross: v[1].as_str().unwrap().parse().unwrap(),
                            net: v[2].as_str().unwrap().parse().unwrap(),
                        },
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>()
                .into(),
        };
        if let Some(controls) = m["one_atom_controls"].as_array() {
            assert_eq!(controls.len(), 2);
            let s = ClState {
                sqrt_price: value(&row["sqrt"]),
                tick: row["tick"].as_i64().unwrap() as i32,
                liquidity: value(&row["liquidity"]).to(),
                fee: 0,
                ticks: &ticks,
                version: Version::V4,
            };
            assert!(
                quote(
                    &s,
                    false,
                    false,
                    row["fee"].as_u64().unwrap() as u32,
                    true,
                    U256::ONE
                )
                .is_none()
            );
            for control in controls {
                assert!(value(&control["quoted"]) > U256::from(i128::MAX as u128));
                assert_eq!(control["received"], "0");
                assert_eq!(control["paid_input"], "0");
            }
        }
        for case in m["cases"]
            .as_array()
            .unwrap()
            .iter()
            .chain(m["independent_usdc"].as_array().into_iter().flatten())
        {
            let mut liquidity = value(&row["liquidity"]).to::<u128>();
            for side in ["buy", "sell"] {
                let Some(order) = case.get(side) else {
                    continue;
                };
                let results = order["results"].as_array().unwrap();
                let calls = order["calls"].as_array().unwrap();
                let token = calls[calls.len() - 10]["to"].as_str().unwrap();
                let dir = m["key"][0].as_str().unwrap().eq_ignore_ascii_case(token);
                let charged = archive["roles"]["WBLT"]
                    .as_str()
                    .unwrap()
                    .eq_ignore_ascii_case(token);
                let slot = words(&results[results.len() - 8]["returnData"]);
                let s = ClState {
                    sqrt_price: slot[0],
                    tick: slot[1].as_limbs()[0] as i32,
                    liquidity,
                    fee: crate::venues::bsc_dex::quote::pool_quote::swap_fee(slot[2].to(), 0, dir),
                    ticks: &ticks,
                    version: Version::V4,
                };
                let fee = slot[3].to();
                let (input, out) = (value(&order["input"]), value(&order["output"]));
                let tested = quote(&s, dir, charged, fee, true, input).unwrap();
                assert_eq!(tested.amount_out, out, "{} {side}", m["pool_id"]);
                let least = quote(&s, dir, charged, fee, false, out).unwrap().amount_in;
                assert!(least <= input);
                assert!(
                    quote(&s, dir, charged, fee, true, least - U256::ONE)
                        .is_none_or(|v| v.amount_out < out)
                );
                let event = results[results.len() - 4]["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| {
                        v["address"] == archive["manager"]
                            && v["topics"][1] == m["pool_id"]
                            && v["topics"][0].as_str().unwrap().starts_with("0x40e9cecb")
                    })
                    .unwrap();
                liquidity = words(&event["data"])[3].to();
                count += 1;
            }
        }
    }
    assert_eq!(count, 28);
}
