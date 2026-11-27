//! Архивы пулов и фабрик EulerSwap сверяют выбор поколения для подписки, поиска и обновления.
use super::creation_topic;
use crate::venues::bsc_dex::pools::{families, per_block};
use alloy_primitives::B256;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn recorded_factories_select_their_own_generation_for_original_and_wallet_module() {
    let roles: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/eulerswap_roles.json"
    ))
    .unwrap();
    let registries: [&[u8]; 2] = [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/eulerswap_base_registry.json.gz"
        ),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/eulerswap_ethereum_registry.json.gz"
        ),
    ];
    for raw in registries {
        let directory: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
        let network = &roles["networks"][directory["chain"].to_string()];
        for (version, name) in [(1, "V1Factory"), (2, "V2Factory")] {
            let row = directory["generations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["version"] == version)
                .unwrap();
            assert!(!row["pools"].as_array().unwrap().is_empty());
            assert_eq!(row["factory"], network[name]["address"]);
            let topic = network[name]["event"]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap();
            for suffix in ["", "_module"] {
                let kind = format!("eulerswap_v{version}{suffix}");
                let f = families::make(&kind, &kind).unwrap();
                assert!(f.variant.euler() && per_block::needs(f));
                assert_eq!(f.variant.euler_v1(), row["registry"] == row["factory"]);
                assert_eq!(
                    f.variant.euler_v2(),
                    row["registry"] == network["V2Registry"]["address"]
                );
                assert_eq!(creation_topic(f), Some(topic));
            }
        }
    }
    let other = families::make("stabull", "stabull").unwrap();
    assert!(!other.variant.euler());
    assert!(!other.variant.euler_v1());
    assert!(!other.variant.euler_v2());
    assert_eq!(creation_topic(other), None);
}
