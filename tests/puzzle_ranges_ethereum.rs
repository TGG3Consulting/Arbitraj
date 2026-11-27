//! Independent Router.query calls on one Ethereum state, including positive amounts next to the physical payout limit.
use alloy_primitives::{B256, U256};
use arb::{
    core::types::pool_vault::{V3Hook, VaultCurve, VaultPoolData, VaultV3, VaultV3Rule},
    venues::bsc_dex::math::balancer_math,
};
use flate2::read::GzDecoder;
use serde_json::Value;

fn archive(input: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(input)).unwrap()
}
fn qty(v: &Value) -> U256 {
    v.to_string().parse().unwrap()
}
fn quantities(v: &Value) -> Vec<U256> {
    v.as_array().unwrap().iter().map(qty).collect()
}

fn market(p: &Value) -> VaultPoolData {
    let v3 = VaultV3 {
        live: quantities(&p["live"]),
        rates: quantities(&p["rates"]),
        min_trade: U256::from(1_000_000_000_000u64),
        rule: VaultV3Rule::PuzzleRanges {
            virtual_balances: quantities(&p["virtual"]),
            enabled: true,
        },
        hook: V3Hook::None,
        open: false,
        via: Vec::new(),
    };
    VaultPoolData {
        pool_id: B256::ZERO,
        balances: quantities(&p["raw"]),
        scaling: quantities(&p["scales"]),
        fee: qty(&p["fee"]),
        curve: VaultCurve::Weighted {
            weights: quantities(&p["weights"]),
        },
        i0: 0,
        i1: 1,
        paused: false,
        v3: Some(Box::new(v3)),
    }
}

#[test]
fn independent_router_calls_match_virtual_pricing_and_physical_settlement_limits() {
    let directory = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz"
    ));
    let quotes = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/ranges_ethereum_quotes.json.gz"
    ));
    assert_eq!(directory["selector"], quotes["selector"]);
    let profile: Value =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let s = &profile
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "ranges-finance" && v["chain_id"] == 1)
        .unwrap()["settings"];
    for (key, role) in [
        ("dex.vault", "vault"),
        ("dex.factory", "factory"),
        ("dex.quoter", "router"),
        ("dex.ranges_hook", "hook"),
    ] {
        assert_eq!(s[key], directory["roles"][role]);
    }
    let mut count = 0;
    let mut differences = Vec::new();
    for p in directory["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["initialized"] == true)
    {
        let market = market(p);
        let pool = p["pool"].as_str().unwrap();
        for r in quotes["markets"][pool].as_array().unwrap() {
            let i = r[0].as_u64().unwrap() as usize;
            let j = r[1].as_u64().unwrap() as usize;
            let amount = qty(&r[3]);
            let want = r[4].is_number().then(|| qty(&r[4]));
            let actual = if r[2] == "In" {
                balancer_math::amount_out(&market, i, j, amount)
            } else {
                balancer_math::amount_in(&market, i, j, amount)
            };
            if actual != want && differences.len() < 8 {
                differences.push(format!(
                    "{pool} {i}->{j} {} {amount}, router {} model {actual:?}",
                    r[2], r[4]
                ));
            }
            count += 1;
        }
    }
    assert_eq!(count, 2750);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn every_market_with_spendable_balances_paid_through_both_settlement_paths() {
    use std::collections::{BTreeMap, BTreeSet};
    let directory = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz"
    ));
    let wallets = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/ranges_ethereum_wallets.json.gz"
    ));
    assert_eq!(directory["selector"], wallets["selector"]);
    let usable: BTreeSet<_> = directory["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| {
            p["initialized"] == true
                && quantities(&p["live"])
                    .iter()
                    .any(|x| *x >= U256::from(1_000_001_000_000u64))
        })
        .map(|p| p["pool"].as_str().unwrap())
        .collect();
    let mut matched: BTreeMap<(&str, &str, u64, &str), (&Value, &Value)> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut paid = 0;
    for r in wallets["cases"].as_array().unwrap() {
        let pool = r["pool"].as_str().unwrap();
        for action in ["buy", "sell"] {
            let leg = &r[action];
            if leg["stage"] != "paid" {
                continue;
            }
            assert_eq!(leg["quote"], leg["credit"]);
            assert_eq!(leg["min_plus_one"], "reverted");
            assert_eq!(leg["expired"], "reverted");
            assert!(qty(&leg["credit"]) > U256::ZERO);
            let direct = r["mode"] == "router";
            let key = (
                pool,
                r["token"].as_str().unwrap(),
                r["size"].as_u64().unwrap(),
                action,
            );
            if direct {
                assert!(
                    matched
                        .insert(key, (&leg["debit"], &leg["credit"]))
                        .is_none()
                );
            } else {
                let (debit, credit) = matched
                    .remove(&key)
                    .expect("compare against the direct wallet");
                assert_eq!((debit, credit), (&leg["debit"], &leg["credit"]));
            }
            seen.insert(pool);
            paid += 1;
        }
    }
    assert_eq!(seen, usable);
    assert_eq!((paid, seen.len(), matched.len()), (240, 7, 0));
}

#[test]
fn ranges_depth_can_exceed_thirty_percent_of_real_input_balance() {
    use arb::venues::bsc_dex::quote::vault_depth;
    let directory = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz"
    ));
    let pool = directory["pools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pool"] == "0xca5fa9ba99a57729d3a6b5b8e84c8417e7632314")
        .unwrap();
    let d = market(pool);
    let (depth, _) = vault_depth::side(&d, 1, 0, 0.90, true).unwrap();
    assert!(depth > d.balances[1] * U256::from(3) / U256::from(10));
    let mut free = d;
    free.fee = U256::ZERO;
    assert!(balancer_math::amount_out(&free, 1, 0, depth).is_some());
}

#[test]
fn smallest_exact_input_reaches_each_historical_target_and_one_less_does_not() {
    use std::collections::BTreeMap;
    let directory = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz"
    ));
    let quotes = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/ranges_ethereum_quotes.json.gz"
    ));
    let mut vectors = BTreeMap::new();
    let mut checked = 0usize;
    for p in directory["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["initialized"] == true)
    {
        let d = market(p);
        let pool = p["pool"].as_str().unwrap();
        let mut calls = BTreeMap::new();
        for q in quotes["markets"][pool]
            .as_array()
            .unwrap()
            .iter()
            .filter(|q| q[4].is_number())
        {
            let i = q[0].as_u64().unwrap() as usize;
            let j = q[1].as_u64().unwrap() as usize;
            let target = if q[2] == "In" { qty(&q[4]) } else { qty(&q[3]) };
            if target.is_zero() {
                continue;
            }
            let (n, out) = balancer_math::for_output(&d, i, j, target)
                .unwrap_or_else(|| panic!("{pool} {i}->{j} unreachable {target} from {q}"));
            assert!(out >= target);
            if q[2] == "In" {
                assert!(n <= qty(&q[3]), "{pool} {i}->{j} paid more on {q}");
            }
            let prior = n.checked_sub(U256::ONE).unwrap();
            let less = balancer_math::amount_out(&d, i, j, prior);
            assert!(
                less.is_none_or(|v| v < target),
                "{pool} {i}->{j} cheaper than {n} for {target}"
            );
            calls.insert((i, j, n), Some(out));
            calls.insert((i, j, prior), less);
            checked += 1;
        }
        if !calls.is_empty() {
            let rows: Vec<_> = calls
                .into_iter()
                .map(|((i, j, n), v)| {
                    serde_json::json!([i, j, n.to_string(), v.map(|credit| credit.to_string())])
                })
                .collect();
            vectors.insert(pool, rows);
        }
    }
    assert!(checked >= 800);
    if let Some(path) = std::env::var_os("ARB_RANGES_BOUNDARIES") {
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "selector": directory["selector"], "markets": vectors, "targets": checked
            }))
            .unwrap(),
        )
        .unwrap();
    }
}
