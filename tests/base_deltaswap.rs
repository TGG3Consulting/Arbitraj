use alloy_primitives::{Address, U256};
use arb::{
    core::types::{PoolState, pool::DeltaFee},
    venues::bsc_dex::{
        math::deltaswap::{after, quote},
        pools::verify::v2_create2,
    },
};
use serde_json::Value;

fn uint(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

fn state(b: &Value, x: &Value) -> (U256, U256, DeltaFee) {
    let l = &x["liquidity_ema"];
    let t = &x["trade_ema"];
    let fees = &b["ds_fee_info"];
    (
        uint(&x["reserves"][0]),
        uint(&x["reserves"][1]),
        DeltaFee {
            router: b["proof"]["router"]["address"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            generation: 1,
            block: b["height"].as_u64().unwrap() + 1,
            liquidity: uint(&l[0]),
            liquidity_block: l[1].as_str().unwrap().parse().unwrap(),
            traded: uint(&t[0]),
            sum: uint(&t[1]),
            trade_block: t[2].as_str().unwrap().parse().unwrap(),
            rate: u8::try_from(fees[0].as_u64().unwrap()).unwrap(),
            threshold: u8::try_from(fees[1].as_u64().unwrap()).unwrap(),
        },
    )
}

#[test]
fn all_registered_pairs_derive_from_factory_and_two_archival_fee_regimes_settle_exactly() {
    let j: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/deltaswap_base.json"
    ))
    .unwrap();
    let base = &j["current"];
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let p = &profiles
        .iter()
        .find(|p| p["id"] == "deltaswap-base" && p["chain_id"] == 8453)
        .unwrap()["settings"];
    assert_eq!(p["dex.kind"], "v2_deltaswap_router");
    assert_eq!(p["dex.factory"], base["proof"]["factory"]["address"]);
    assert_eq!(p["dex.router"], base["proof"]["router"]["address"]);
    assert_eq!(
        p["dex.module_runtime_hashes"][0],
        base["proof"]["router"]["runtime"]
    );
    assert_eq!(p["dex.pool_code_hashes"][0], base["proof"]["pair_init"]);
    let factory = p["dex.factory"].as_str().unwrap().parse().unwrap();
    let init = p["dex.pool_code_hashes"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let listed = base["pools"].as_array().unwrap();
    assert_eq!(listed.len(), 35);
    for pool in listed {
        let token = |i: usize| pool["tokens"][i].as_str().unwrap().parse().unwrap();
        let address: Address = pool["pool"].as_str().unwrap().parse().unwrap();
        assert_eq!(address, v2_create2(factory, token(0), token(1), init));
    }
    for b in [base, &j["historical"]] {
        let mut rates = std::collections::BTreeSet::new();
        for group in b["wallet"].as_array().unwrap() {
            let mut expected = None;
            for (i, x) in group.as_array().unwrap().iter().enumerate() {
                let (r0, r1, f) = state(b, x);
                if let Some(prior) = expected.take() {
                    assert_eq!(
                        prior,
                        PoolState::DeltaSwap {
                            reserve0: r0,
                            reserve1: r1,
                            fee: Box::new(f.clone())
                        }
                    );
                }
                let dx = uint(&x["amount"]);
                let dy = uint(&x["out"]);
                assert_eq!(uint(&x["amount_spent"]), dx);
                assert_eq!(uint(&x["amount_received"]), dy);
                assert!(x["gas"].as_u64().unwrap() > 0);
                assert!(
                    x["next_revert"]
                        .as_str()
                        .unwrap()
                        .ends_with("INSUFFICIENT_OUTPUT_AMOUNT")
                );
                assert_ne!(x["gamma_pool"], b["proof"]["router"]["address"]);
                let (trade, next) = after(r0, r1, &f, i == 0, dx).unwrap();
                assert_eq!((trade.amount_in, trade.amount_out), (dx, dy));
                let inverse = quote(r0, r1, &f, i == 0, false, dy).unwrap().amount_in;
                assert!(
                    inverse <= dx
                        && quote(r0, r1, &f, i == 0, true, inverse).unwrap().amount_out >= dy
                );
                if let Some(less) = quote(r0, r1, &f, i == 0, true, inverse - U256::ONE) {
                    assert!(less.amount_out < dy);
                }
                rates.insert(x["invariant_fee"].as_u64().unwrap());
                expected = Some(next);
            }
        }
        let expected = if b["ds_fee_info"][1] == 0 {
            vec![3]
        } else {
            vec![0, 3]
        };
        assert_eq!(rates.into_iter().collect::<Vec<_>>(), expected);
    }
}
