//! Измерения зарегистрированных пар KEWL, в том числе фактический расход входа и граница выплаты.
#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;
use alloy_primitives::Address;
use serde_json::Value;

fn addr(s: &Value) -> Address {
    s.as_str().unwrap().parse().unwrap()
}
fn gas(case: &Value) -> u64 {
    u64::from_str_radix(case["gas"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

#[test]
fn actual_create_nonce_role_and_deployed_code_match_each_profile_field() {
    let j: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/kewlswap_base.json"
    ))
    .unwrap();
    let s = &first_run_profiles::named(8453, "kewlswap-base").unwrap()["settings"];
    assert_eq!(s["dex.kind"], "v2_diamond_create");
    assert_eq!(s["dex.router"], s["dex.factory"]);
    assert_eq!(s["dex.v2_fee_pct"], "0.3");
    assert!(s.get("dex.pool_code_hashes").is_none());
    assert_eq!(s["dex.pair_runtime_hash"], j["origin"]["pair_runtime"]);
    assert_eq!(
        s["dex.factory_runtime_hash"],
        j["execution"]["creator"]["runtime"]
    );
    assert_eq!(
        s["dex.router_runtime_hash"],
        j["execution"]["router"]["runtime"]
    );
    assert_eq!(j["origin"]["account_nonce"], 5);
    let rows = j["pools"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    let factory = addr(&s["dex.factory"]);
    for row in rows {
        let nonce = row["creation_nonce"].as_u64().unwrap();
        assert!(nonce > 0 && nonce < 5);
        assert_eq!(factory.create(nonce), addr(&row["pool"]));
    }
    assert_ne!(
        s["dex.proxy_implementations"][0],
        s["dex.proxy_implementations"][1]
    );
    for (i, role) in [(0, "creator"), (1, "router")] {
        let r = &j["execution"][role];
        let facet = &s["dex.proxy_implementations"][i];
        assert_eq!(
            s["dex.proxy_runtime_hashes"][i],
            r["implementations"][facet.as_str().unwrap()]
        );
        assert!(
            r["selectors"]
                .as_object()
                .unwrap()
                .values()
                .all(|v| v == facet)
        );
    }
}

#[test]
fn native_and_v5_match_at_wallet_and_both_revert_one_unit_above() {
    let j: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/kewlswap_base.json"
    ))
    .unwrap();
    let mut legs = 0;
    for m in j["execution"]["markets"].as_array().unwrap() {
        let Some(rows) = m["legs"].as_array() else {
            assert_eq!(m["pool"], "0x5c972c206470b4975c95ab7272d3d57077e74c37");
            assert!(
                m["reason"]
                    .as_str()
                    .unwrap()
                    .contains("INSUFFICIENT_OUTPUT_AMOUNT")
            );
            continue;
        };
        for group in rows.chunks_exact(2) {
            assert_eq!(group[0]["mode"], "v5");
            assert_eq!(group[1]["mode"], "router");
            assert_eq!(group[0]["size"], group[1]["size"]);
            for leg in ["buy", "sell"] {
                let (v, r) = (&group[0][leg], &group[1][leg]);
                assert!(gas(&v["exact"]) < gas(&r["exact"]));
                for key in ["spent", "received"] {
                    assert_eq!(v["exact"][key], r["exact"][key]);
                }
                for x in [v, r] {
                    assert_eq!(x["exact"]["status"], "0x1");
                    assert_eq!(x["above"]["status"], "0x0");
                    assert_eq!(x["above"]["spent"], "0");
                    assert_eq!(x["above"]["received"], "0");
                    legs += 1;
                }
            }
        }
    }
    assert_eq!(legs, 24);
}

#[test]
fn last_unit_of_usdt_blocks_buys_but_funded_sales_still_settle() {
    let j: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/kewlswap_base.json"
    ))
    .unwrap();
    let markets = j["execution"]["markets"].as_array().unwrap();
    let row = markets.iter().find(|m| m["one_way"].is_array()).unwrap();
    let pool = j["pools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["pool"] == row["pool"])
        .unwrap();
    assert_eq!(pool["reserves"][1], "1");
    assert_eq!(row["unavailable_buy"]["reserve_out"], "1");
    assert_eq!(
        row["funding"]["path"].as_array().unwrap().last(),
        Some(&pool["tokens"][1])
    );
    let rows = row["one_way"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    for (i, leg) in rows.iter().enumerate() {
        let (size, net) = if i < 2 {
            ("1", "9098765")
        } else {
            ("2", "12137765")
        };
        assert_eq!(leg["mode"], if i % 2 == 0 { "v5" } else { "router" });
        assert_eq!(leg["size"], size);
        assert_eq!(leg["sell"]["exact"]["spent"], size);
        assert_eq!(leg["sell"]["exact"]["received"], net);
        assert_eq!(leg["sell"]["above"]["status"], "0x0");
        assert_eq!(leg["sell"]["above"]["spent"], "0");
        assert_eq!(leg["sell"]["above"]["received"], "0");
    }
}
