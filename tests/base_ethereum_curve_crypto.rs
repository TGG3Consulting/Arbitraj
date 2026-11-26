//! Котировки Curve Crypto-NG на закреплённых блоках Base и Ethereum. Проверяет декодирование
//! ответов штатного загрузчика и точное совпадение формулы с get_dy в обе стороны на двух объёмах.

mod curve_crypto_common;

use std::str::FromStr;

use alloy_primitives::Address;
use arb::venues::bsc_dex::pools::{crypto_variant, load_crypto};
use curve_crypto_common::{answers, ladder};
use serde_json::Value;

#[test]
fn market_quotes_match_the_normal_state_loader() {
    let captures = [
        include_str!("data/first_run_dex_2026-11-26/curve_base.json"),
        include_str!("data/first_run_dex_2026-11-26/curve_ethereum.json"),
    ];
    for capture in captures {
        let doc: Value = serde_json::from_str(capture).unwrap();
        for (kind, n) in [("twocrypto", 2), ("tricrypto", 3)] {
            let family = &doc["families"][kind];
            assert_eq!(family["status"], "quoted");
            let math = |role: &str| {
                family["math"][role]["address"]
                    .as_str()
                    .map(|a| Address::from_str(a).unwrap())
                    .into_iter()
                    .collect::<Vec<_>>()
            };
            let ng = math("math");
            let old = math("math_v2_0_0");
            let mut seen = 0;
            for row in family["pools"].as_array().unwrap() {
                if row["status"] != "quoted" {
                    continue;
                }
                let pool = Address::from_str(row["pool"].as_str().unwrap()).unwrap();
                let raw = load_crypto::Raw {
                    pool,
                    n,
                    now: doc["timestamp"].as_u64().unwrap(),
                    got: answers(row, &load_crypto::calls(pool, n)),
                };
                let (_, mut data, actual_math) = load_crypto::decode(&raw).unwrap();
                data.math = crypto_variant::variant(n, actual_math, &old, &ng).unwrap();
                assert_eq!(ladder(kind, pool, row, &data), 4);
                seen += 1;
            }
            assert!(seen >= 1, "{kind}, chain {}", doc["chain_id"]);
        }
    }
}
