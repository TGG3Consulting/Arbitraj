use super::{ArbExecutor, ExecCall, llamma_debit};
use crate::venues::bsc_dex::{
    check::simulate::SimLog,
    pools::{family_variant, llamma::ILlama},
};
use alloy::sol_types::{SolCall, SolEvent, SolValue};
use alloy_primitives::{Address, Bytes, U256};
use flate2::read::GzDecoder;
use serde_json::Value;

fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn pancake_bsc_exchange_keeps_the_original_input() {
    let receipt: Value = serde_json::from_str(include_str!(
        "../../../../../tests/data/bsc_pancake_stable_receipt_2026-11-27.json"
    ))
    .unwrap();
    assert_eq!(receipt["chainId"], 56);
    let logs: Vec<SimLog> = serde_json::from_value(receipt["logs"].clone()).unwrap();
    let pool = address(&receipt["pool"]);
    let raw = logs
        .iter()
        .find(|log| {
            log.address == pool
                && log.topics.first() == Some(&ILlama::TokenExchange::SIGNATURE_HASH)
        })
        .unwrap();
    let (i, sold, j, bought) = <(U256, U256, U256, U256)>::abi_decode(&raw.data).unwrap();
    assert_eq!((i, j), (U256::ZERO, U256::ONE));
    assert!(!sold.is_zero() && !bought.is_zero());
    let executor = Address::from_word(raw.topics[1]);
    let tokens = &receipt["poolTokens"];
    for family in [
        "pancake_stableswap",
        "lista_stableswap",
        "lista_stableswap_proxy",
    ] {
        let call = ExecCall {
            first_step_llamma: family_variant::of(family).llamma(),
            executor,
            token_in: address(&tokens[0]),
            token_out: address(&tokens[1]),
            amount_in: sold,
            min_out: bought,
            deadline: U256::MAX,
            native: None,
            gated: vec![],
            steps: vec![ArbExecutor::Step {
                kind: 9,
                pool,
                tokenIn: address(&tokens[0]),
                tokenOut: address(&tokens[1]),
                shareBps: 10000,
                data: (i, j).abi_encode_params().into(),
            }],
        };
        assert!(!call.first_step_llamma);
        assert_eq!(llamma_debit(&call, executor, &logs), None);
    }
}

#[test]
fn matching_refund_logs_change_the_debit_only_for_the_registered_llamma_family() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../../tests/data/first_run_dex_2026-11-26/llamma_ethereum_wallets.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    let mut partial = 0;
    let mut full = 0;
    for row in archive["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["mode"] == "v5")
    {
        let paid = &row["result"];
        let Some(raw_logs) = paid.get("logs") else {
            continue;
        };
        let tx = &paid["trade"];
        let data: Bytes = tx["data"].as_str().unwrap().parse().unwrap();
        let signed = ArbExecutor::executeCall::abi_decode(&data).unwrap();
        let wallet = address(&tx["from"]);
        let logs: Vec<SimLog> = serde_json::from_value(raw_logs.clone()).unwrap();
        let mut call = ExecCall {
            first_step_llamma: true,
            executor: address(&tx["to"]),
            token_in: signed.tokenIn,
            token_out: signed.tokenOut,
            amount_in: signed.amountIn,
            min_out: signed.minOut,
            deadline: signed.deadline,
            steps: signed.steps,
            native: None,
            gated: vec![],
        };
        let spent: U256 = paid["spent"].as_str().unwrap().parse().unwrap();
        if spent < call.amount_in {
            partial += 1;
            assert_eq!(llamma_debit(&call, wallet, &logs), Some(spent));
            call.first_step_llamma = family_variant::of("pancake_stableswap").llamma();
            assert_eq!(llamma_debit(&call, wallet, &logs), None);
        } else {
            full += 1;
            assert_eq!(spent, call.amount_in);
            assert_eq!(llamma_debit(&call, wallet, &logs), None);
        }
    }
    assert_eq!((partial, full), (2, 20));
}
