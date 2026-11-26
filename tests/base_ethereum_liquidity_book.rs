//! Проверяет зарегистрированные пары Liquidity Book и ответ getSwapOut, который берёт бот.
use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use arb::venues::bsc_dex::pools::verify_lb::clone_implementation;
use arb::venues::bsc_dex::quote::quoter_lb;
use serde_json::Value;

#[test]
fn recorded_pairs_match_the_selected_implementations_and_quotes() {
    for source in [
        include_str!("data/first_run_dex_2026-11-26/liquidity_book_base.json"),
        include_str!("data/first_run_dex_2026-11-26/liquidity_book_ethereum.json"),
    ] {
        let d: Value = serde_json::from_str(source).unwrap();
        let implementation = Address::from_str(d["implementation"].as_str().unwrap()).unwrap();
        let mut proven = 0;
        for r in d["rows"].as_array().unwrap() {
            if r["status"] != "quoted" {
                continue;
            }
            let code = Bytes::from_str(r["clone"].as_str().unwrap()).unwrap();
            assert_eq!(clone_implementation(&code), Some(implementation));
            assert_eq!(clone_implementation(&code[..code.len() - 1]), None);
            let calls: Vec<_> = d["calls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|a| a["to"] == r["pool"] && a["signature"] == "getSwapOut(uint128,bool)")
                .collect();
            assert_eq!(calls.len(), 4);
            for (q, c) in r["quotes"].as_array().unwrap().iter().zip(calls) {
                let raw = Bytes::from_str(c["response"].as_str().unwrap()).unwrap();
                let out = U256::from_str(q["amount_out"].as_str().unwrap()).unwrap();
                assert_eq!(quoter_lb::decode(&raw), Some(out));
                assert!(out > U256::ZERO);
            }
            proven += 1;
        }
        assert!(proven > 0, "chain {}", d["chain_id"]);
    }
}
