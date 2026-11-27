//! Фактические списания и выплаты TempleAdapter, сохранённые архивной EVM на Ethereum.
use super::temple_wallet::{Key, Proof};
use alloy_primitives::{Address, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;
fn addr(v: &Value) -> Address {
    serde_json::from_value(v.clone()).unwrap()
}
fn amount(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn temple_protected_wallet_receipts_bind_assets_recipient_and_limits() {
    let raw: &[u8] = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/temple_ethereum_wallets.json.gz"
    );
    let a: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
    let wallet = addr(&a["wallet"]);
    let key = Key {
        module: Address::from_slice(
            &keccak256(b"Arbitraj Temple protected module Ethereum").as_slice()[12..],
        ),
        pair: addr(&a["pair"]),
        router: addr(&a["router"]),
        temple: addr(&a["assets"][0]),
        stable: addr(&a["stable"]),
        defend: addr(&a["defend_stable"]),
    };
    let mut verified = 0;
    for case in a["cases"].as_array().unwrap() {
        if case["mode"] == "router" {
            assert_eq!(amount(&case["buy"]["return_data"]), U256::ZERO);
        }
        if case["mode"] != "protected" {
            continue;
        }
        for leg in ["buy", "sell"] {
            let row = &case[leg];
            let minimum = amount(&row["minimum"]);
            let mut p = Proof {
                key,
                payer: wallet,
                input: addr(&row["input"]),
                output: addr(&row["output"]),
                amount: amount(&row["amount"]),
                minimum,
                deadline: U256::ONE,
            };
            let logs =
                serde_json::from_value::<Vec<crate::venues::bsc_dex::check::simulate::SimLog>>(
                    row["logs"].clone(),
                )
                .unwrap();
            let net = Some((p.amount, minimum));
            assert_eq!(p.measured(&logs), net);
            assert_eq!(
                row["delta"][if leg == "buy" { 0 } else { 1 }]
                    .as_str()
                    .unwrap(),
                minimum.to_string()
            );
            assert!(row["too_high"].is_object() && row["expired"].is_object());
            p.minimum += U256::ONE;
            assert_eq!(p.measured(&logs), None);
            p.minimum = minimum;
            p.payer = key.router;
            assert_eq!(p.measured(&logs), None);
            p.payer = wallet;
            p.output = key.defend;
            assert_eq!(p.measured(&logs), None);
            verified += 1;
        }
    }
    assert_eq!(verified, 4);
}
