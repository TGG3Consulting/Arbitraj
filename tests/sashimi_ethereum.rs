//! Wallet payments and router custody in the canonical Ethereum Sashimi registry.
use alloy_primitives::{U256, keccak256};
use serde_json::Value;
fn archive(raw: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap()
}
fn units(v: &Value) -> U256 {
    let s = v.as_str().unwrap();
    if let Some(hex) = s.strip_prefix("0x") {
        U256::from_str_radix(hex, 16).unwrap()
    } else {
        s.parse().unwrap()
    }
}
#[test]
fn pair_books_and_cash_are_distinct_and_router_settlements_reconcile() {
    let d = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/sashimi_ethereum_registry.json.gz"
    ));
    let paid = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/sashimi_ethereum_wallets.json.gz"
    ));
    assert_eq!(d["selector"], paid["selector"]);
    assert_eq!(d["state_root"], paid["state_root"]);
    let markets = d["pools"].as_array().unwrap();
    assert_eq!(markets.len(), 83);
    for m in markets {
        assert_eq!(m["router_ledger"], m["reserves"]);
        assert_eq!(m["pair_balances"], serde_json::json!(["0", "0"]));
    }
    let transfer = format!("{:#x}", keccak256("Transfer(address,address,uint256)"));
    let swap = format!(
        "{:#x}",
        keccak256("Swap(address,uint256,uint256,uint256,uint256,address)")
    );
    let (mut fills, mut recorded) = (0, 0);
    for row in paid["wallets"].as_array().unwrap() {
        let m = markets.iter().find(|m| m["pool"] == row["pool"]).unwrap();
        let other = m["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| **t != row["input"])
            .unwrap();
        for (leg, input, output) in [
            (&row["buy"], &row["input"], other),
            (&row["sell"], other, &row["input"]),
        ] {
            if leg["stage"] != "paid" {
                continue;
            }
            let r = leg["results"].as_array().unwrap();
            assert_eq!(r.len(), 9);
            assert_eq!(r[2]["status"], "0x0");
            assert_eq!(r[3]["status"], "0x0");
            assert_eq!(r[0]["returnData"], r[4]["returnData"]);
            assert_eq!(r[1]["returnData"], r[5]["returnData"]);
            assert_eq!(r[6]["status"], "0x1");
            let debit = units(&r[0]["returnData"]) - units(&r[7]["returnData"]);
            let credit = units(&r[8]["returnData"]) - units(&r[1]["returnData"]);
            assert_eq!(debit, units(&leg["debit"]));
            assert_eq!(credit, units(&leg["credit"]));
            assert!(credit > U256::ZERO);
            let owner =
                U256::from_str_radix(row["payer"].as_str().unwrap().trim_start_matches("0x"), 16)
                    .unwrap();
            let mut debited = U256::ZERO;
            let mut credited = U256::ZERO;
            let mut swaps = 0;
            for log in r[6]["logs"].as_array().unwrap() {
                let topics = log["topics"].as_array().unwrap();
                if topics.first().and_then(Value::as_str) == Some(swap.as_str())
                    && log["address"] == m["pool"]
                {
                    swaps += 1;
                }
                if topics.len() != 3 || topics[0] != transfer {
                    continue;
                }
                if log["address"] == *input && units(&topics[1]) == owner {
                    debited += units(&log["data"]);
                }
                if log["address"] == *output && units(&topics[2]) == owner {
                    credited += units(&log["data"]);
                }
            }
            assert_eq!(swaps, 1);
            assert_eq!(debited, debit);
            if credited == credit {
                recorded += 1
            } else {
                assert_eq!(credited, U256::ZERO);
                assert_eq!(row["sell"]["stage"], "allowance_or_quote");
            }
            fills += 1;
        }
    }
    assert_eq!((fills, recorded), (129, 113));
}
