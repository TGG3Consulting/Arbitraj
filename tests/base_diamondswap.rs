//! Сверка профиля первого запуска и двух путей DiamondSwap на каноническом архиве Base.
#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;

use alloy_primitives::{Address, U256};
use arb::venues::bsc_dex::pools::verify::v2_create2;
use serde_json::Value;

fn proof() -> Value {
    serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/diamondswap_base.json"
    ))
    .unwrap()
}

fn addr(value: &Value) -> Address {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn profile_attests_each_facet_and_all_sixteen_registered_pairs() {
    let j = proof();
    let row = first_run_profiles::named(8453, "diamondswap").unwrap();
    let s = &row["settings"];
    assert_eq!(s["dex.kind"], "v2_diamond_router");
    assert_eq!(j["height"], 62_262_756);
    assert_eq!(j["fee"], 20);
    assert_eq!(s["dex.v2_fee_method"], "swapFee()");
    assert_eq!(s["dex.v2_fee_denominator"], 10_000);
    let f = &j["factory"];
    let r = &j["router"];
    assert_eq!(f["runtime"], r["runtime"]);
    for (i, field, shell) in [(0, "factory", f), (1, "router", r)] {
        assert_eq!(s[format!("dex.{field}")], shell["address"]);
        assert_eq!(s[format!("dex.{field}_runtime_hash")], shell["runtime"]);
        assert_eq!(shell["loupe"], shell["address"]);
        let facet = &s["dex.proxy_implementations"][i];
        assert_eq!(
            shell["implementations"][facet.as_str().unwrap()],
            s["dex.proxy_runtime_hashes"][i]
        );
        assert!(
            shell["selectors"]
                .as_object()
                .unwrap()
                .values()
                .all(|actual| actual == facet)
        );
    }
    assert_ne!(
        s["dex.proxy_implementations"][0],
        s["dex.proxy_implementations"][1]
    );
    assert_eq!(s["dex.pool_code_hashes"][0], j["creation"]["hash"]);
    let init = j["creation"]["hash"].as_str().unwrap().parse().unwrap();
    let rows = j["markets"].as_array().unwrap();
    assert_eq!(rows.len(), 16);
    for p in rows {
        assert_eq!(
            v2_create2(
                addr(&f["address"]),
                addr(&p["tokens"][0]),
                addr(&p["tokens"][1]),
                init
            ),
            addr(&p["pool"])
        );
    }
}

fn gas(v: &Value) -> u64 {
    u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

#[test]
fn funded_native_and_v5_settle_the_same_wallet_at_its_exact_floor() {
    let j = proof();
    let rows = j["wallets"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    for pair in rows.chunks_exact(2) {
        assert_eq!(pair[0]["mode"], "v5");
        assert_eq!(pair[1]["mode"], "router");
        assert_eq!(pair[0]["size"], pair[1]["size"]);
        for direction in ["buy", "sell"] {
            let v5 = &pair[0][direction]["exact"];
            let native = &pair[1][direction]["exact"];
            for k in ["spent", "received", "approval_gas"] {
                assert_eq!(v5[k], native[k]);
            }
            assert_eq!(gas(&v5["gas"]) - gas(&native["gas"]), 8317);
            for route in pair {
                let leg = &route[direction];
                assert_eq!(leg["exact"]["status"], "0x1");
                assert_eq!(leg["above"]["status"], "0x0");
                assert_eq!(leg["above"]["spent"], "0");
                assert_eq!(leg["above"]["received"], "0");
            }
        }
    }
    let sample = j["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["cases"].is_array())
        .unwrap();
    for example in sample["cases"].as_array().unwrap() {
        for (way, expected) in [("buy", "bought"), ("sell", "returned")] {
            assert_eq!(example["pair_boundary"][way]["max"], example[expected]);
            assert_eq!(example["pair_boundary"][way]["next_revert"], "0xcd6fabbb");
        }
    }
    for trial in j["curve"].as_array().unwrap() {
        let direction = usize::from(trial["tokens"][0] != sample["tokens"][0]);
        let value = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let reserves = &sample["reserves"];
        let amount = value(&trial["amount"]);
        let net = amount * U256::from(10_000 - trial["fee"].as_u64().unwrap());
        let predicted = net * value(&reserves[1 - direction])
            / (value(&reserves[direction]) * U256::from(10_000) + net);
        assert_eq!(predicted, value(&trial["out"]));
        if trial["fee"] == j["fee"] {
            assert_eq!(trial["out"], trial["live"]);
        }
    }
}
