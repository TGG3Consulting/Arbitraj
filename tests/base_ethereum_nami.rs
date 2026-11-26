//! Сравнивает направленную математику Nami с реальной выплатой и учитывает сторонние Swap из callback.
use alloy_primitives::{U256, keccak256};
use arb::venues::bsc_dex::{
    math::swap_algebra::{self, AlgebraState, NextTick},
    pools::ticks::{TickData, TickMap},
};
use serde_json::Value;
use std::collections::BTreeMap;

fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn map(m: &Value) -> TickMap {
    let w = &m["tick_window"];
    let ticks: BTreeMap<_, _> = w["ticks"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(i, t)| {
            (
                i.parse().unwrap(),
                TickData {
                    gross: t["gross"].as_str().unwrap().parse().unwrap(),
                    net: t["net"].as_str().unwrap().parse().unwrap(),
                },
            )
        })
        .collect();
    TickMap {
        spacing: 1,
        ticks: ticks.into(),
        words: (
            w["lower_word"].as_i64().unwrap() as i16,
            w["upper_word"].as_i64().unwrap() as i16,
        ),
    }
}

#[test]
fn directional_math_and_touched_sites_agree_with_real_active_hooks() {
    let swap = format!(
        "{:#x}",
        keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)")
    );
    for (raw, expected, with_external) in [
        (
            include_str!("data/first_run_dex_2026-11-26/nami_base.json"),
            8,
            0,
        ),
        (
            include_str!("data/first_run_dex_2026-11-26/nami_ethereum.json"),
            4,
            4,
        ),
    ] {
        let archive: Value = serde_json::from_str(raw).unwrap();
        let mut count = 0;
        let mut external = 0;
        for m in archive["markets"].as_array().unwrap() {
            assert_eq!(m["fee_config"][0], 0);
            assert_eq!(m["fee_config"][1], 0);
            assert_eq!(m["fee_source"][1], false);
            assert_eq!(m["fee_bounds"][1], 0);
            let ticks = map(m);
            for c in m["cases"].as_array().unwrap() {
                let zfo = c["source"] == m["tokens"][0];
                let state = AlgebraState {
                    sqrt_price: number(&m["state"][0]),
                    tick: m["state"][1].as_str().unwrap().parse().unwrap(),
                    liquidity: m["liquidity"].as_str().unwrap().parse().unwrap(),
                    fee: m["directional"][usize::from(!zfo)].as_u64().unwrap() as u32,
                    ticks: &ticks,
                    next: NextTick::Linked,
                };
                let calculated = swap_algebra::swap(&state, zfo, true, number(&c["dx"])).unwrap();
                let r = &c["results"];
                let n = |k: &str| number(&r[k]["returnData"]);
                let official = hex::decode(
                    r["official"]["returnData"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("0x"),
                )
                .unwrap();
                assert_eq!(
                    arb::venues::bsc_dex::pools::nami::decoded(&official),
                    Some(calculated.amount_out)
                );
                assert!(calculated.complete);
                assert_eq!(calculated.amount_in, number(&c["dx"]));
                assert_eq!(calculated.amount_out, number(&c["out"]));
                assert_eq!(n("in_before") - n("in_after"), calculated.amount_in);
                assert_eq!(n("out_after") - n("out_before"), calculated.amount_out);
                assert_eq!(r["above"]["status"], "0x0");
                assert!(n("executor_in").is_zero() && n("executor_out").is_zero());
                let swaps: Vec<_> = r["exact"]["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|l| l["topics"][0].as_str() == Some(swap.as_str()))
                    .collect();
                assert!(swaps.iter().any(|l| l["address"] == m["pool"]));
                if swaps.iter().any(|l| l["address"] != m["pool"]) {
                    external += 1;
                }
                count += 1;
            }
        }
        assert_eq!((count, external), (expected, with_external));
    }
}

#[test]
fn supernova_four_custom_registries_bind_every_pool_and_plugin() {
    use alloy_primitives::{Address, B256};
    use std::collections::{BTreeMap, BTreeSet};
    let archive: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/supernova_ethereum_directory.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let p = &profiles
        .iter()
        .find(|p| p["chain_id"] == 1 && p["id"] == "supernova-cl")
        .unwrap()["settings"];
    let physical = archive["deployer"]
        .as_str()
        .unwrap()
        .parse::<Address>()
        .unwrap();
    let init = archive["init"].as_str().unwrap().parse::<B256>().unwrap();
    assert_eq!(p["dex.pool_code_hashes"][0], archive["init"]);
    assert_eq!(archive["selector"]["requireCanonical"], true);
    let mut counts = BTreeMap::new();
    let mut flags = BTreeMap::new();
    let mut addresses = BTreeSet::new();
    for row in archive["markets"].as_array().unwrap() {
        let custom = &row["custom"];
        assert!(
            p["dex.custom_deployers"]
                .as_array()
                .unwrap()
                .contains(custom)
        );
        let mut salt = Vec::with_capacity(96);
        for v in [custom, &row["tokens"][0], &row["tokens"][1]] {
            salt.extend_from_slice(
                v.as_str()
                    .unwrap()
                    .parse::<Address>()
                    .unwrap()
                    .into_word()
                    .as_slice(),
            );
        }
        let addr = row["pool"].as_str().unwrap().parse::<Address>().unwrap();
        assert!(addresses.insert(addr));
        assert_eq!(physical.create2(keccak256(salt), init), addr);
        assert_eq!(row["registered"], true);
        assert_eq!(row["slots"], 9);
        assert_eq!(row["template"], p["dex.plugin_runtime_hashes"][1]);
        assert!(row["created"]["block"].as_u64().unwrap() <= archive["block"].as_u64().unwrap());
        *counts.entry(custom.as_str().unwrap()).or_insert(0usize) += 1;
        *flags
            .entry(row["flags"].as_u64().unwrap())
            .or_insert(0usize) += 1;
    }
    let mut groups: Vec<_> = counts.into_values().collect();
    groups.sort_unstable();
    assert_eq!(groups, [4, 7, 16, 25]);
    assert_eq!(flags, BTreeMap::from([(87, 42), (215, 10)]));
}
