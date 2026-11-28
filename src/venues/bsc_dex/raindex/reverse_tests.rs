//! Все активные ордера Base V6 и обе стороны USDC на архивном блоке 62 262 756.
use super::*;
use std::collections::{HashMap, HashSet};

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn address(row: &Value, field: &str) -> Address {
    from_value(row[field].clone()).unwrap()
}
fn positive(row: &Value) -> bool {
    row["live"] == true
        && ["cap", "ratio"].iter().all(|k|
            Quantity::float(from_value(row[*k].clone()).unwrap()).is_some())
}

#[test]
fn registered_reverse_io_and_funded_usdc_market_sales_at_the_same_block() {
    let orders = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_indexed_orders.json.gz"
    ));
    let quotes = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_quotes.json.gz"
    ));
    let funded = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_usdc_sales.json.gz"
    ));
    assert_eq!(quotes["anchor"], funded["selector"]);
    assert_eq!(quotes["book"], funded["book"]);
    let usdc: Address = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap();
    let mut pairs = HashMap::new();
    let mut registered = HashMap::new();
    for entry in orders.as_array().unwrap() {
        let hash: B256 = from_value(entry["orderHash"].clone()).unwrap();
        let encoded: Bytes = from_value(entry["orderBytes"].clone()).unwrap();
        let order = wire::Order::decode(6, &encoded, hash).unwrap();
        // Нулевой vault V6 расходует внешний баланс maker. В этих 506 ордерах все выходы используют именные vault.
        let wire::Order::Float(6, stored) = &order else { panic!("expected published V6 order") };
        assert!(stored.validOutputs.iter().all(|io| io.vaultId != B256::ZERO));
        let (inputs, outputs) = order.pairs();
        for token in &inputs {
            for out in &outputs {
                if *out == usdc && *token != usdc {
                    *registered.entry(*token).or_insert(0usize) += 1;
                }
            }
        }
        assert!(pairs.insert(hash, (inputs, outputs)).is_none());
    }
    assert_eq!((pairs.len(), registered.len(), registered.values().sum::<usize>()), (506, 58, 281));
    let mut probed = HashSet::new();
    for entry in quotes["entries"].as_array().unwrap() {
        for q in entry["quotes"].as_array().unwrap() {
            if address(q, "receive") == usdc {
                assert!(probed.insert((entry["hash"].as_str().unwrap(),
                    q["i"].as_u64().unwrap(), q["j"].as_u64().unwrap())));
            }
        }
    }
    let mut accepted = HashSet::new();
    let mut assets = HashSet::new();
    let mut outcomes = (0, 0);
    for row in funded["census"].as_array().unwrap().iter()
        .filter(|r| address(r, "receive") == usdc)
    {
        let hash: B256 = from_value(row["order"].clone()).unwrap();
        let i = row["i"].as_u64().unwrap();
        let j = row["j"].as_u64().unwrap();
        assert!(probed.remove(&(row["order"].as_str().unwrap(), i, j)));
        let (inputs, outputs) = &pairs[&hash];
        assert_eq!(inputs[i as usize], address(row, "pay"));
        assert_eq!(outputs[j as usize], usdc);
        assets.insert(inputs[i as usize]);
        if positive(row) {
            assert!(accepted.insert((hash, i, j)));
        } else if row["status"] == "0x0" {
            outcomes.0 += 1;
        } else {
            outcomes.1 += 1;
        }
    }
    assert!(probed.is_empty());
    assert_eq!((assets.len(), outcomes, accepted.len()), (55, (83, 5), 5));
    let mut counts = (0, 0, 0);
    for sale in funded["sales"].as_array().unwrap() {
        let q = &sale["quote"];
        let hash = from_value(q["order"].clone()).unwrap();
        assert!(accepted.remove(&(hash, q["i"].as_u64().unwrap(), q["j"].as_u64().unwrap())));
        let token = q["pay"].as_str().unwrap();
        let purchases = funded["purchases"][token].as_array().unwrap();
        let tests = sale["tests"].as_array().unwrap();
        if tests.is_empty() {
            assert_eq!(sale["available"], "0");
            assert!(purchases.iter().all(|r| r["status"] == "0x0" && r["before"] == r["after"]));
            counts.2 += 1;
        } else {
            assert!(purchases.iter().any(|r| r["status"] == "0x1" && r["net"] == sale["available"]));
            for attempt in tests {
                assert_eq!(attempt["status"], "0x0");
                assert_eq!(attempt["before"], attempt["after"]);
                assert_eq!(attempt["fromBalance"], sale["available"]);
                if attempt["protected"] == false {
                    assert!(attempt["error"]["message"].as_str().unwrap().contains("Min trade amount."));
                    counts.0 += 1;
                } else {
                    counts.1 += 1;
                }
            }
        }
    }
    assert!(accepted.is_empty());
    assert_eq!(counts, (3, 3, 4));
}
