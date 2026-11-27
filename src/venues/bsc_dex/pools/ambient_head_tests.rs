use super::*;
#[path = "ambient_silent_receipt.rs"]
mod silent_receipt;
use crate::core::types::{
    DexRoute, Pool, PoolRef, PoolState, RouteHop, RouteLeg, RouteMode, VenueId,
    ambient::{Market, State},
};
use crate::venues::bsc_dex::pools::{families, registry::Entry, registry_frozen::Want};
use alloy_primitives::{Address, Bytes, U256};
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use serde_json::Value;
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn entry(reg: &Registry, archive: &Value, row: &Value, n: u64, h: B256) -> Entry {
    let family = families::make("ambient", "ambient_ethereum").unwrap();
    let market = Market {
        base: addr(&row["base"]),
        quote: addr(&row["quote"]),
        index: U256::from(row["index"].as_u64().unwrap()),
    };
    let dex = addr(&archive["roles"]["dex"]);
    let wrapped = addr(&archive["wrapped"]);
    let [input, output] = market.tokens(wrapped).unwrap();
    let bytes: Bytes = row["impact"]["answer"].as_str().unwrap().parse().unwrap();
    let point = crate::venues::bsc_dex::pools::ambient::decode(&bytes, true).unwrap();
    assert_eq!(
        point.amount_in,
        row["asked"].as_str().unwrap().parse::<U256>().unwrap()
    );
    let levels = [vec![point], vec![]];
    Entry {
        family,
        pool: Pool {
            venue: VenueId("ethereum".into()),
            family: family.id.into(),
            kind: family.kind,
            reference: PoolRef::Singleton {
                manager: dex,
                id: market.id(),
            },
            token0: input.min(output),
            token1: input.max(output),
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::Ambient(Box::new(State {
                stamp: reg.ambient_stamp(n, h),
                module: None,
                dex,
                market,
                wrapped,
                impact: addr(&archive["roles"]["impact"]),
                levels,
                partial_cap: [None, None],
            })),
            block: n,
            received_at: chrono::Utc::now(),
        },
        ticker: "asset".into(),
        coin: input,
        usdt: output,
        key: None,
        ticks: None,
        bins: None,
        synced_block: n,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}
fn points(e: &Entry) -> bool {
    matches!(&e.pool.state,PoolState::Ambient(s) if s.points(e.coin).is_some_and(|x|!x.is_empty()))
}
#[test]
fn delayed_second_market_and_reorg_cannot_restore_a_priced_snapshot() {
    let a: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ambient_ethereum_silent_swap.json.gz"
        )[..],
    ))
    .unwrap();
    let reg = Registry::default();
    let parent = &a["headers"][0];
    let child = &a["headers"][1];
    let n = 26_492_698;
    assert_eq!(parent["number"], format!("{n:#x}"));
    assert_eq!(child["number"], format!("{:#x}", n + 1));
    assert_eq!(parent["hash"], child["parentHash"]);
    let h = parent["hash"].as_str().unwrap().parse().unwrap();
    let next = child["hash"].as_str().unwrap().parse().unwrap();
    silent_receipt::check(&a, child);
    let [older, newer] = a["states"].as_array().unwrap().as_slice() else {
        panic!("two headers")
    };
    assert_ne!(older[0]["price"]["answer"], newer[0]["price"]["answer"]);
    assert_eq!(older[1]["price"]["answer"], newer[1]["price"]["answer"]);
    let first = entry(&reg, &a, &older[0], n, h);
    let second = entry(&reg, &a, &older[1], n, h);
    let r1 = first.pool.reference.clone();
    let r2 = second.pool.reference.clone();
    assert!(!reg.put_observed(first.clone()));
    assert!(reg.watches_ambient());
    assert!(reg.ambient_head(Some((n, h))));
    reg.put(first.clone());
    reg.put(second.clone());
    let pin = reg.observed_ambient().unwrap();
    let route = DexRoute {
        mode: RouteMode::Graph,
        gas_units: 0,
        block: n,
        executable: true,
        exec_note: None,
        legs: vec![RouteLeg {
            share: Decimal::ONE,
            expected_out: Decimal::ONE,
            hops: [&first, &second]
                .into_iter()
                .map(|e| RouteHop {
                    family: e.family.id.into(),
                    kind: e.pool.kind,
                    pool: e.pool.reference.clone(),
                    fee_mode: None,
                })
                .collect(),
        }],
    };
    let keys = vec![vec![
        (r1.clone(), first.coin, first.usdt),
        (r2.clone(), second.coin, second.usdt),
    ]];
    let snap = reg.freeze(&Want {
        coin: first.coin,
        usdt: first.usdt,
        mids: &[],
        wbnb: None,
        paths: &keys,
    });
    let frozen = Registry::frozen_from(&snap, None);
    assert!(frozen.route_was_current(&route, Some(&pin)));
    let out = |e: &Entry| {
        let PoolState::Ambient(s) = &e.pool.state else {
            unreachable!()
        };
        s.points(e.coin).unwrap()[0].amount_out
    };
    assert_eq!(
        (out(&first), out(&second)),
        (U256::from(1_613_303_444), U256::from(996_759))
    );
    let replacement = B256::repeat_byte(5);
    reg.ambient_head(Some((n + 1, next)));
    assert!(!points(&first) && !points(&second));
    assert!(!frozen.route_was_current(&route, Some(&pin)));
    assert!(!reg.get(&r1).unwrap().valid && !reg.get(&r2).unwrap().valid);
    let a1 = entry(&reg, &a, &newer[0], n + 1, next);
    let a2 = entry(&reg, &a, &newer[1], n + 1, next);
    assert_eq!(
        (out(&a1), out(&a2)),
        (U256::from(1_613_289_705), U256::from(996_759))
    );
    assert!(reg.put_observed(a1.clone()));
    assert!(!reg.get(&r2).unwrap().valid);
    assert!(!reg.put_observed(first));
    assert!(reg.get(&r1).unwrap().valid);
    reg.ambient_head(Some((n + 1, replacement)));
    assert!(!reg.put_observed(a2) && !reg.put_observed(a1));
    assert!(!reg.get(&r1).unwrap().valid && !reg.get(&r2).unwrap().valid);
    reg.put(entry(&reg, &a, &newer[0], n + 1, replacement));
    reg.put(entry(&reg, &a, &newer[1], n + 1, replacement));
    let renewed = reg.observed_ambient().unwrap();
    assert!(reg.route_was_current(&route, Some(&renewed)));
    reg.ambient_head(None);
    assert!(!reg.route_was_current(&route, Some(&renewed)));
    assert!(!points(&second));
}
