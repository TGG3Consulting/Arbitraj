use super::{
    profile::{Profile, Role},
    receipt::Saved,
    wire::{Order, Version},
};
use crate::venues::bsc_dex::exec::signed_call::Erc20Call;
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, U256};
use flate2::read::GzDecoder;
use serde_json::{Value, json};
#[test]
fn both_event_generations_and_all_eighteen_paid_forms() {
    let mut count = 0;
    for bytes in [
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/airswap_base_orders.json.gz"
        )[..],
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/airswap_ethereum_orders.json.gz"
        )[..],
    ] {
        let j: Value = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
        for (key, rows) in j["funded_fixture_orders"].as_object().unwrap() {
            let version = if key == "v5" {
                Version::V5
            } else {
                Version::V4
            };
            let role = &j["roles"][if key == "v5" { "swap" } else { "legacy" }];
            let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
            let role = Role {
                address: address(&role["address"]),
                hash: role["hash"].as_str().unwrap().parse().unwrap(),
            };
            for row in rows.as_array().unwrap() {
                let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
                let event = logs.iter().find(|l| l.address() == role.address).unwrap();
                let nonce = U256::from_be_bytes(event.topics()[1].0);
                let maker = Address::from_slice(&event.topics()[2].as_slice()[12..]);
                let input = address(&row["input"]);
                let output = address(&row["output"]);
                let amt = |k: &str| row[k].as_str().unwrap().parse::<U256>().unwrap();
                let owner=row.get("payer").map(address).unwrap_or_else(||{
                    let c=logs.iter().find(|l|l.address()==input && l.topics().first()==Some(&crate::venues::bsc_dex::node::abi::IERC20::Transfer::SIGNATURE_HASH)).unwrap();
                    Address::from_slice(&c.topics()[1].as_slice()[12..])
                });
                let module = row.get("module").map(|v| Role {
                    address: address(v),
                    hash: j["modules"][key]["runtimeKeccak"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                });
                let call = Erc20Call {
                    chain: j["chain_id"].as_u64().unwrap(),
                    payer: owner,
                    module: module.unwrap_or(role).address,
                    calldata: B256::ZERO,
                };
                let proof = Saved {
                    call,
                    profile: Profile {
                        chain: call.chain,
                        version,
                        exchange: role,
                        module,
                    },
                    maker,
                    nonce,
                    digest: B256::ZERO,
                    expiry: 0,
                    input,
                    output,
                    amount: amt("spent"),
                    quoted: amt("received"),
                    fee: U256::from(j["terms"][key]["fee"].as_u64().unwrap()),
                    minimum: amt("received"),
                };
                assert_eq!(proof.amounts(&logs), Some((amt("spent"), amt("received"))));
                let mut duplicate = logs.clone();
                duplicate.push(event.clone());
                assert!(proof.amounts(&duplicate).is_none());
                let mut wrong = proof;
                wrong.nonce += U256::ONE;
                assert!(wrong.amounts(&logs).is_none());
                count += 1;
            }
        }
    }
    assert_eq!(count, 18);
}
#[test]
fn fee_zero_is_valid_and_nonce_signed_for_other_taker_differs() {
    let input = Address::repeat_byte(1);
    let output = Address::repeat_byte(2);
    let base = json!({"signerWallet":Address::repeat_byte(3),"signerToken":output,"senderToken":input,
        "signerAmount":"11","senderAmount":"12","protocolFee":"0","nonce":"8","expiry":"100","v":"27","r":B256::ZERO,"s":B256::ZERO});
    let order = Order::read(&base).unwrap();
    assert_ne!(
        order.digest(B256::ZERO, input),
        order.digest(B256::ZERO, Address::ZERO)
    );
    let mut wrong = base;
    wrong["senderToken"] = json!(output);
    assert!(Order::read(&wrong).is_none());
}

#[test]
fn measured_input_rebate_does_not_expand_authorized_budget() {
    let budget = rust_decimal::Decimal::from(100);
    assert!(super::routing::within_input(
        U256::from(100_000_000u64),
        6,
        budget
    ));
    assert!(!super::routing::within_input(
        U256::from(100_000_001u64),
        6,
        budget
    ));
}
