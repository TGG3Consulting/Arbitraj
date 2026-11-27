//! Офлайн повтор публичной квитанции и локальные одноразовые подписи. Сеть не вызывается.
use super::{fill, wire::*};
use crate::venues::bsc_dex::send::receipt::Receipt;
use alloy::{
    primitives::{Address, Bytes, U256},
    signers::{SignerSync, local::PrivateKeySigner},
    sol_types::SolCall,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Sample {
    transaction: Transaction,
    receipt: Receipt,
}
#[derive(Deserialize)]
struct Transaction {
    to: Address,
    input: Bytes,
}

fn samples() -> Vec<(Sample, SwapIntent)> {
    let raw = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_base_historical_receipts.json.gz"
    );
    let cases: Vec<Sample> =
        serde_json::from_reader(flate2::read::GzDecoder::new(&raw[..])).unwrap();
    cases
        .into_iter()
        .map(|case| {
            let batch =
                executeSwapIntentsCall::abi_decode_validate(&case.transaction.input).unwrap();
            (case, batch.signedIntents.into_iter().next().unwrap().intent)
        })
        .collect()
}

#[test]
fn mined_batch_binds_signer_nonce_router_path_and_wallet_deltas() {
    let cases = samples();
    let expected = [
        (638_852_115u64, 379_929_577_043_376_464u64, 10_000u64, 0u64),
        (379_929_577_043_376_464, 638_870_441, 5_944_578_692_847, 0),
        (3476, 2_053_827, 17, 1),
    ];
    assert_eq!(cases.len(), expected.len());
    for ((mut case, order), (spent, received, fee, path_credit)) in cases.into_iter().zip(expected)
    {
        let tx = &case.transaction;
        assert!(order.present_in(tx.to, &tx.input));
        assert!(!order.present_in(Address::repeat_byte(2), &tx.input));
        let mut wrong = order.clone();
        wrong.nonce += U256::ONE;
        assert!(!wrong.present_in(tx.to, &tx.input));
        assert!(fill::confirmed(&wrong, tx.to, &case.receipt).is_none());
        let f = fill::confirmed(&order, tx.to, &case.receipt).unwrap();
        assert_eq!(f.input_to_router, U256::from(spent));
        assert_eq!(f.output_from_router, U256::from(received));
        assert_eq!(f.fee_in_input, U256::from(fee));
        assert_eq!(f.input_from_path, U256::from(path_credit));
        case.receipt.logs.extend(case.receipt.logs.clone());
        assert!(fill::confirmed(&order, tx.to, &case.receipt).is_none());
    }
}

#[test]
fn ephemeral_signature_is_bound_to_gross_input_minimum_expiry_and_own_account() {
    let (case, mut order) = samples().pop().unwrap();
    let signer = PrivateKeySigner::random();
    order.user = signer.address();
    order.recipient = signer.address();
    let router = case.transaction.to;
    let sig = signer.sign_hash_sync(&order.digest(8453, router)).unwrap();
    let body = order.submission(router, &sig).unwrap();
    assert_eq!(body["amountIn"], order.amountIn.to_string());
    assert!(body["nonce"].is_number() && body["deadline"].is_number());
    let hop = &order.path[0];
    let markets = [(hop.pool, hop.tokenIn, hop.tokenOut)];
    assert!(order.matches(order.user, &markets, order.deadline.to::<u64>()));
    assert!(!order.matches(order.user, &markets, order.deadline.to::<u64>() + 1));
    assert!(!order.matches(order.user, &[], 0));
    let mut changed = order.clone();
    changed.minAmountOut += U256::ONE;
    assert!(changed.submission(router, &sig).is_none());
    changed = order.clone();
    changed.recipient = router;
    assert!(!changed.matches(order.user, &markets, 0));
    assert!(changed.submission(router, &sig).is_none());
}

#[test]
fn real_two_hop_and_reverse_cbbtc_trades_bind_each_hop_and_charge_per_hop() {
    use serde_json::Value;
    let bytes = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_base_path_history.json.gz"
    );
    let history: Value = serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).unwrap();
    let rows = history["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 6);
    let raw =
        |row: &Value, key: &str| U256::from_str_radix(row[key].as_str().unwrap(), 10).unwrap();
    let mut two_hop = 0;
    for row in rows {
        let case: Sample = serde_json::from_value(row.clone()).unwrap();
        let tx = &case.transaction;
        let batch = executeSwapIntentsCall::abi_decode_validate(&tx.input).unwrap();
        let order = &batch.signedIntents[0].intent;
        assert!(order.present_in(tx.to, &tx.input));
        let fill = fill::confirmed(order, tx.to, &case.receipt).unwrap();
        assert_eq!(fill.input_to_router, raw(row, "amount"));
        assert_eq!(fill.output_from_router, raw(row, "net"));
        assert_eq!(fill.fee_in_input, raw(row, "fee"));
        assert_eq!(
            (
                fill.input_from_path,
                fill.other_input_credit,
                fill.other_output_credit
            ),
            (U256::ZERO, U256::ZERO, U256::ZERO)
        );
        let price = batch
            .cexPriceData
            .iter()
            .find(|p| p.token == order.tokenIn)
            .unwrap()
            .priceInWad;
        assert_eq!(price, raw(row, "signed_price"));
        let usd = raw(row, "usd_fee");
        let decimals = u8::try_from(row["decimals"].as_u64().unwrap()).unwrap();
        let count = order.path.len();
        assert_eq!(
            super::price::input_fee(usd, price, decimals, count),
            Some(fill.fee_in_input)
        );
        if count == 2 {
            two_hop += 1;
            assert_eq!(order.path[0].tokenOut, order.path[1].tokenIn);
            let mut wrong = order.clone();
            wrong.path.reverse();
            assert!(!wrong.present_in(tx.to, &tx.input));
            let late = super::price::input_fee(usd * U256::from(2), price, decimals, 1).unwrap();
            assert_eq!(late + U256::ONE, fill.fee_in_input);
        } else {
            assert_eq!(count, 1);
        }
    }
    assert_eq!(two_hop, 4);
}
