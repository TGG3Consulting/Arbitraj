use super::*;
use serde_json::Value;
use std::io::Read;

fn archive(raw: &[u8]) -> Value {
    let mut buf = Vec::new();
    flate2::read::GzDecoder::new(raw)
        .read_to_end(&mut buf)
        .unwrap();
    serde_json::from_slice(&buf).unwrap()
}
fn raw(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn state(r: &Value) -> LevState {
    LevState {
        index: r["index"].as_u64().unwrap(),
        amm: address(&r["amm"]),
        stable: address(&r["stable"]),
        lp: address(&r["pool"]),
        lt: address(&r["lt"]),
        oracle: address(&r["oracle"]),
        collateral: raw(&r["collateral"]),
        debt: raw(&r["debt"]),
        fee: raw(&r["fee"]),
        price: raw(&r["price"]),
        precision: U256::from(10u64).pow(U256::from(18 - r["decimals"].as_u64().unwrap())),
        cash: [raw(&r["physical"][0]), raw(&r["physical"][1])],
        killed: r["killed"].as_bool().unwrap(),
        limits: if r["index"].as_u64().unwrap() < 7 {
            Limits::Always
        } else {
            Limits::RelaxImproving
        },
    }
}
#[test]
fn original_views_and_debt_boundary_match_both_deployed_versions() {
    let snapshots = archive(include_bytes!(
        "../../../tests/data/first_run_dex_2026-11-26/yield_basis_lev_state.json.gz"
    ));
    let originals = archive(include_bytes!(
        "../../../tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz"
    ));
    assert_eq!(snapshots["selector"], originals["selector"]);
    let (mut views, mut paid, mut unsafe_max) = (0, 0, 0);
    for row in snapshots["rows"].as_array().unwrap() {
        let s = state(row);
        let amt = &originals["venues"][row["amm"].as_str().unwrap()];
        assert_eq!(row["runtime"], amt["code"]["keccak256"]);
        assert_eq!(s.x0(s.collateral, s.debt, false), Some(raw(&row["x0"])));
        assert!(s.amount(s.stable, s.debt + U256::ONE).is_none());
        assert!(s.amount(s.stable, s.debt).is_some_and(|v| !v.is_zero()));
        assert!(s.swap(s.stable, s.debt).is_none());
        for q in row["quotes"].as_array().unwrap() {
            let input = if q["side"] == 0 { s.stable } else { s.lp };
            assert_eq!(
                s.amount(input, raw(&q["qty"])),
                q["out"].as_str().map(|v| v.parse().unwrap())
            );
            views += 1;
        }
        for c in amt["cases"].as_array().unwrap() {
            let input = address(&c["source"]);
            let qty = raw(&c["amount"]);
            let fill = s.swap(input, qty);
            if c["stage"] == "pool rejected" {
                assert_eq!(c["reason"], "Unsafe max");
                assert!(fill.is_none(), "{c}");
                unsafe_max += 1;
            } else {
                assert_eq!(c["stage"], "paid");
                assert_eq!(c["spent"], c["amount"]);
                assert_eq!(c["received"], c["preview"]);
                // Minting LP during the funded setup can change the oracle before this trade.
                let local = fill.unwrap();
                let inverse = s.forward_for_output(input, local.received).unwrap();
                assert!(inverse.spent <= qty && inverse.received >= local.received);
                if inverse.spent > U256::ZERO {
                    assert!(s.amount(input, inverse.spent - U256::ONE).unwrap() < local.received);
                }
                paid += 1;
            }
        }
    }
    assert_eq!((views, paid, unsafe_max), (66, 28, 16));
}

#[test]
fn usdt_routes_pay_both_physical_hops_at_the_same_parent() {
    let book = archive(include_bytes!(
        "../../../tests/data/first_run_dex_2026-11-26/yield_basis_usdt_routes.json.gz"
    ));
    let origins = archive(include_bytes!(
        "../../../tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz"
    ));
    assert_eq!(book["selector"], origins["selector"]);
    let assets = book["delta_order"].as_array().unwrap();
    assert_eq!(book["coins"], serde_json::json!([assets[0], assets[1]]));
    assert_ne!(assets[0], assets[1]);
    let mut samples = std::collections::BTreeSet::new();
    for r in book["rows"].as_array().unwrap() {
        let sell = r["source"] == assets[2];
        let (src, dst) = if sell { (2, 0) } else { (0, 2) };
        assert_eq!(r["source"], assets[src]);
        assert_eq!(r["target"], assets[dst]);
        let d = r["wallet_deltas"].as_array().unwrap();
        assert_eq!(d.len(), 9);
        assert_eq!(
            d[src].as_str().unwrap(),
            format!("-{}", r["spent"].as_str().unwrap())
        );
        assert_eq!(d[dst], r["received"]);
        assert_eq!(d[1], "0");
        assert!(d[3..].iter().all(|n| n == "0"));
        assert!(raw(&r["received"]) > U256::ZERO);
        let cash = &r[if sell { "received" } else { "spent" }];
        assert_ne!(
            raw(&r["bridge_crvusd_raw"]),
            raw(cash) * U256::from(1_000_000_000_000u64)
        );
        assert_eq!(
            r["steps"],
            if sell {
                serde_json::json!([17, 4])
            } else {
                serde_json::json!([4, 17])
            }
        );
        assert_eq!(r["limits"], serde_json::json!(["0x0", "0x0", "0x1"]));
        assert!(r["gas"].as_u64().unwrap() > 0 && r["approval_gas"].as_u64().unwrap() > 0);
        assert!(samples.insert((
            r["index"].as_u64().unwrap(),
            r["role"].as_str().unwrap(),
            sell,
            raw(&r["spent"])
        )));
    }
    assert_eq!(samples.len(), 16);
}
