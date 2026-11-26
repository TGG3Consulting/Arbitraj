//! Sobal Base. Входы взяты из контрактов на закреплённом блоке, выходы — из Vault.queryBatchSwap.
use alloy_primitives::{Address, B256, U256};
use arb::core::types::{PoolState, VaultCurve, pool_vault_v2::V2Curve};
use arb::venues::bsc_dex::{math::balancer_math, pools::load_balancer};
use serde_json::Value;

fn uint(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

fn numbers(value: &Value) -> Vec<U256> {
    value.as_array().unwrap().iter().map(uint).collect()
}

#[test]
fn model_matches_both_sides_of_every_funded_registered_sobal_curve() {
    let directory: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/sobal_base_directory.json"
    ))
    .unwrap();
    let archive: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/sobal_base_quotes.json"
    ))
    .unwrap();
    assert_eq!(archive["selector"], directory["selector"]);
    let pools = directory["pools"].as_array().unwrap();
    assert_eq!(pools.len(), 26);
    for (nonce, p) in pools.iter().enumerate() {
        assert_eq!(p["nonce"].as_u64().unwrap() as usize, nonce);
    }
    let (mut positive, mut zero) = (0, 0);
    for m in archive["markets"].as_array().unwrap() {
        let registered = &pools[m["nonce"].as_u64().unwrap() as usize];
        assert_eq!(m["id"], registered["id"]);
        assert_eq!(m["tokens"], registered["tokens"]);
        assert_eq!(m["balances"], registered["balances"]);
        let tokens: Vec<Address> = serde_json::from_value(m["tokens"].clone()).unwrap();
        let id: B256 = serde_json::from_value(m["id"].clone()).unwrap();
        let scales: Vec<_> = m["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| U256::from(10).pow(U256::from(18 - d.as_u64().unwrap())))
            .collect();
        let curve = match m["kind"].as_str().unwrap() {
            "weighted" => VaultCurve::WeightedV2 {
                weights: numbers(&m["weights"]),
            },
            "composable_stable" => VaultCurve::V2(Box::new(V2Curve::StableRated {
                amp: uint(&m["amp"]),
                bpt: Some(m["bpt"].as_u64().unwrap() as u8),
                factors: numbers(&m["factors"]),
                v2: true,
                alt: None,
            })),
            t => panic!("unexpected funded Sobal curve {t}"),
        };
        for q in m["cases"].as_array().unwrap() {
            let i = q["i"].as_u64().unwrap() as usize;
            let j = q["j"].as_u64().unwrap() as usize;
            let PoolState::VaultPool(s) = load_balancer::state(
                id,
                &tokens,
                numbers(&m["balances"]),
                scales.clone(),
                uint(&m["fee"]),
                m["paused"].as_bool().unwrap(),
                curve.clone(),
                (tokens[i], tokens[j]),
            )
            .unwrap() else {
                panic!("expected VaultPool");
            };
            let actual = balancer_math::amount_out(&s, i, j, uint(&q["amount"]));
            let expected = uint(&q["output"]);
            if expected.is_zero() {
                assert!(actual.is_none_or(|v| v.is_zero()), "{q}");
                zero += 1;
            } else {
                assert_eq!(actual, Some(expected), "market {} {q}", m["pool"]);
                positive += 1;
            }
        }
    }
    assert_eq!((positive, zero), (70, 8));
}
