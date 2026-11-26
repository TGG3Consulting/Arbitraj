use alloy_primitives::{Address, U256};
use arb::{
    core::types::PoolState,
    venues::bsc_dex::{
        math::v2,
        pools::{oasis, verify::v2_create2},
    },
};
use serde_json::Value;

fn uint(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn registry_and_fee_cache_match_native_wallet_and_v5_minimums() {
    let native: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/oasisswap_base.json"
    ))
    .unwrap();
    let v5: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/oasisswap_v5_base.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let cfg = &profiles
        .iter()
        .find(|r| r["id"] == "oasisswap-base" && r["chain_id"] == 8453)
        .unwrap()["settings"];
    assert_eq!(cfg["dex.kind"], "oasisswap_v2_router");
    assert!(cfg.get("dex.router").is_none());
    assert_eq!(
        cfg["dex.module_runtime_hashes"][0],
        v5["module_runtime_hash"]
    );
    assert_eq!(cfg["dex.factory"], v5["factory"]);
    for (address, key) in [
        ("dex.factory", "dex.factory_runtime_hash"),
        ("dex.quoter", "dex.router_runtime_hash"),
    ] {
        assert_eq!(
            native["origin"][cfg[address].as_str().unwrap()]["runtime"],
            cfg[key]
        );
    }
    assert_eq!(cfg["dex.oasis_mev_slot"], native["controls"]["mev_slot"]);
    assert_eq!(cfg["dex.pool_code_hashes"][0], native["init"]);
    assert_eq!(
        native["controls"]["mev_controller"],
        Address::ZERO.to_string()
    );
    assert_eq!(native["controls"]["estimator"], Address::ZERO.to_string());
    assert_eq!(native["controls"]["approved"], true);
    assert_eq!(native["selector"], v5["selector"]);
    assert_eq!(native["state_root"], v5["state_root"]);
    let pools = native["pools"].as_array().unwrap();
    assert_eq!(pools.len(), 60);
    let factory = cfg["dex.factory"].as_str().unwrap().parse().unwrap();
    let init = native["init"].as_str().unwrap().parse().unwrap();
    for p in pools {
        let add = |i: usize| p["tokens"][i].as_str().unwrap().parse().unwrap();
        assert_eq!(
            v2_create2(factory, add(0), add(1), init),
            p["pool"].as_str().unwrap().parse::<Address>().unwrap()
        );
        for i in [0usize, 1] {
            assert_eq!(
                uint(&p["reserves"][i]) + uint(&p["fee_cache"][i]),
                uint(&p["balances"][i])
            );
        }
    }
    let mut failed_sells = 0;
    let mut accepted = 0;
    let mut fot_reverts = 0;
    for case in v5["cases"].as_array().unwrap() {
        let p = pools.iter().find(|p| p["pool"] == case["pool"]).unwrap();
        let i = usize::from(case["asset"] != p["tokens"][0]);
        let (r0, r1) = (uint(&p["reserves"][0]), uint(&p["reserves"][1]));
        let dx = uint(&case["size"]);
        let fee = p["fee"].as_u64().unwrap();
        let share = u32::try_from(p["protocol_share"].as_u64().unwrap()).unwrap();
        let ratio = (U256::from(10000 - fee), U256::from(10000));
        let (rin, rout) = if i == 0 { (r0, r1) } else { (r1, r0) };
        let buy = v2::amount_out(dx, rin, rout, ratio).unwrap();
        assert_eq!(buy, uint(&case["buy"]["accepted"]["received"]));
        let next = oasis::after(
            r0,
            r1,
            share,
            u32::try_from(fee * 100).unwrap(),
            i == 0,
            dx,
            buy,
        )
        .unwrap();
        let PoolState::Oasis {
            reserve0, reserve1, ..
        } = next
        else {
            panic!()
        };
        let (new_in, new_out) = if i == 0 {
            (reserve0, reserve1)
        } else {
            (reserve1, reserve0)
        };
        let sell = &case["sell"];
        let read = if sell.get("accepted").is_some() {
            &sell["accepted"]
        } else {
            &sell["minimum_one"]
        };
        assert_eq!(
            rin + dx - new_in,
            uint(&read["fee_cache"][i]) - uint(&p["fee_cache"][i])
        );
        let returned = v2::amount_out(buy, new_out, new_in, ratio).unwrap();
        assert_eq!(returned, uint(&read["native_quote"]));
        for (direction, input) in [("buy", dx), ("sell", buy)] {
            let leg = &case[direction];
            if let Some(fill) = leg.get("accepted") {
                assert_eq!(uint(&fill["floor"]), uint(&fill["received"]));
                assert_eq!(input, uint(&fill["spent"]));
                let strict = &leg["strict"];
                assert_eq!(uint(&strict["floor"]), uint(&fill["received"]) + U256::ONE);
                assert_eq!(strict["status"], "0x0");
                assert_eq!(uint(&strict["spent"]), U256::ZERO);
                accepted += 1;
            } else {
                assert_eq!(direction, "sell");
                assert_eq!(leg["minimum_one"]["status"], "0x0");
                assert_eq!(uint(&leg["minimum_one"]["floor"]), U256::ONE);
                failed_sells += 1;
            }
        }
    }
    for s in native["samples"].as_array().unwrap() {
        for x in s["cases"].as_array().unwrap() {
            if x["method"] == "swapExactTokensForTokensSupportingFeeOnTransferTokens"
                && (x["buy"]["failed"] == "swap" || x["sell"]["failed"] == "swap")
            {
                fot_reverts += 1;
            }
        }
    }
    assert_eq!((accepted, failed_sells), (6, 2));
    assert!(fot_reverts > 0);
}
