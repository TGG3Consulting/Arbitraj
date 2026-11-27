//! Архивные выплаты Ethereum проверены GitHub eth_simulateV1. Все продажи используют результат покупки того же кошелька.
use flate2::read::GzDecoder;
use serde_json::Value;
fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn units(value: &Value) -> alloy_primitives::U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn baseline_both_routes_pay_only_the_bought_position() {
    let data = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/baseline_ethereum_wallets.json.gz"
    ));
    let dir = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/baseline_ethereum_directory.json.gz"
    ));
    assert_eq!(data["selector"], dir["selector"]);
    let markets = data["markets"].as_array().unwrap();
    assert_eq!(markets.len(), 6);
    for m in markets {
        let cases = m["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 4);
        for c in cases {
            for name in ["buy", "sell"] {
                let a = &c[name];
                assert_eq!(a["stage"], "accepted");
                assert_eq!(a["paid"], a["input"]);
                assert_eq!(a["net"], a["quoted"]);
                assert!(units(&a["net"]) > alloy_primitives::U256::ZERO);
                let r = a["results"].as_array().unwrap();
                assert_eq!(
                    [
                        &r[r.len() - 5]["status"],
                        &r[r.len() - 4]["status"],
                        &r[r.len() - 3]["status"]
                    ],
                    ["0x0", "0x0", "0x1"]
                );
            }
            assert_eq!(c["sell"]["input"], c["buy"]["net"]);
        }
    }
}
#[test]
fn degen_protected_measures_each_leg_and_keeps_directional_refusals() {
    let data = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/degenswap_ethereum_protected.json.gz"
    ));
    let dir = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/degenswap_ethereum_directory.json.gz"
    ));
    assert_eq!(data["selector"], dir["selector"]);
    for mode in ["module", "v5"] {
        let cases: Vec<_> = data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["mode"] == mode)
            .collect();
        assert_eq!(cases.len(), 66);
        assert_eq!(cases.iter().filter(|r| r["buy"].is_object()).count(), 46);
        assert_eq!(cases.iter().filter(|r| r["sell"].is_object()).count(), 2);
        for c in cases {
            for name in ["buy", "sell"] {
                let leg = &c[name];
                if leg.is_null() {
                    continue;
                }
                let x = &leg["exact"];
                let before = x["before"].as_array().unwrap();
                let after = x["after"].as_array().unwrap();
                assert_eq!(units(&before[0]) - units(&after[0]), units(&x["spent"]));
                assert_eq!(units(&after[1]) - units(&before[1]), units(&x["received"]));
                assert_eq!(before[2..], after[2..]);
                assert!(leg["above"].is_object() && leg["expired"].is_object());
            }
            if c["sell"].is_object() {
                assert_eq!(c["sell"]["exact"]["spent"], c["buy"]["exact"]["received"]);
            }
        }
    }
}

#[test]
fn degen_executor_preserves_the_full_wallet_debit_after_taxed_funding() {
    let data = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/degenswap_ethereum_funded.json.gz"
    ));
    for mode in ["module", "v5"] {
        let cases: Vec<_> = data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["mode"] == mode)
            .collect();
        let mut fills = 0;
        let mut discounted = 0;
        for c in &cases {
            for side in ["buy", "sell"] {
                let leg = &c[side];
                if !leg.is_object() {
                    continue;
                }
                let x = &leg["exact"];
                let before = x["before"].as_array().unwrap();
                let after = x["after"].as_array().unwrap();
                let paid = units(&x["spent"]);
                let funded = units(&x["module_spent"]);
                assert!(funded > alloy_primitives::U256::ZERO && funded <= paid);
                assert_eq!(paid, units(&before[0]) - units(&after[0]));
                assert_eq!(units(&x["received"]), units(&after[1]) - units(&before[1]));
                assert_eq!(before[2..], after[2..]);
                assert!(leg["above"].is_object() && leg["expired"].is_object());
                if funded < paid {
                    discounted += 1;
                }
                fills += 1;
            }
        }
        assert_eq!(cases.len(), 66);
        assert_eq!(fills, 92);
        if mode == "v5" {
            assert!(discounted > 0)
        } else {
            assert_eq!(discounted, 0)
        }
    }
}

#[test]
fn external_ethereum_ids_bind_to_one_profile_and_cypher_page_to_the_matching_factory() {
    let profiles: Value =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let index: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/ethereum_indexer_bindings.json"
    ))
    .unwrap();
    for (external, profile) in [
        ("cypher", "cypher-v4-ethereum"),
        ("degenswap", "degenswap-ethereum"),
        ("integral_size", "integral-size-ethereum"),
    ] {
        let claimants: Vec<_> = profiles
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| {
                p["chain_id"] == 1
                    && p["settings"].get("dex.indexer_id").unwrap_or(&p["id"]) == external
            })
            .collect();
        assert_eq!(claimants.len(), 1);
        assert_eq!(claimants[0]["id"], profile);
    }
    let directory = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/cypher_v4_ethereum_directory.json.gz"
    ));
    let rows = index["cypher"]["pages"][0]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 19);
    assert!(
        index["cypher"]["pages"][1]["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for row in rows {
        assert_eq!(row[1], "cypher");
        assert!(
            directory["markets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["pool"] == row[0])
        );
    }
}


mod native_common;

/// Проверяет именно штатную точку входа. Без отдельной диспетчеризации обе площадки
/// попадали в построитель обычного Algebra-квотера и не доходили до проверки валют.
#[tokio::test]
async fn caller_bound_algebra_uses_its_own_quoter_before_generic_dispatch() {
    use alloy_primitives::{Address,U256};
    use arb::{gateway::errors::ErrorCode, venues::bsc_dex::quote::quoter};
    let ctx=native_common::ctx("caller-bound-algebra").await;
    for kind in ["algebra_integral_cypher","algebra_integral_theorem"] {
        let entry=native_common::entry(kind,None);
        let error=quoter::out_at(&ctx,&entry,Address::ZERO,U256::ONE,"latest").await.unwrap_err();
        assert_eq!(error.code,ErrorCode::InvalidAddress);
    }
}
