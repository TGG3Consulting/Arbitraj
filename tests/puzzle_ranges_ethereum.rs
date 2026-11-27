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
        let market = VaultPoolData {
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
        };
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
