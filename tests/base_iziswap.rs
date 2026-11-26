//! Сверяет код и calldata VirtualQuoter по двусторонним ответам узла и штатного квотера iZiSwap.
use alloy_primitives::{Address, Bytes, U256, keccak256};
use arb::venues::bsc_dex::quote::virtual_quoter;
use serde_json::Value;
use std::str::FromStr;

#[test]
fn virtual_i_zi_quotes_match_canonical_pool_calls() {
    let d: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/iziswap_base.json"
    ))
    .unwrap();
    assert_eq!(
        d["virtual_address"],
        format!("{:#x}", virtual_quoter::address())
    );
    assert_eq!(
        d["virtual_runtime_keccak"],
        format!("{:#x}", keccak256(virtual_quoter::runtime().unwrap()))
    );
    let mut steps = 0;
    for p in d["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        assert_eq!(p["quotes"].as_array().unwrap().len(), 4);
        for q in p["quotes"].as_array().unwrap() {
            let amount = U256::from_str(q["amount_in"].as_str().unwrap()).unwrap();
            let expected = U256::from_str(q["native_out"].as_str().unwrap()).unwrap();
            let call = virtual_quoter::data_izi(pool, q["token_in"] == p["x"], amount);
            assert_eq!(q["data"], format!("{call}"));
            let raw = Bytes::from_str(q["response"]["result"].as_str().unwrap()).unwrap();
            assert_eq!(virtual_quoter::decode(&raw, amount), Some(Ok(expected)));
            steps += 1;
        }
    }
    assert_eq!(steps, 12);
}
