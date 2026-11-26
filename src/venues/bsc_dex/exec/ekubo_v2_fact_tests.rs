//! The signed wire and native reconciliation are checked against the archived official Router and Core.
use super::Proof;
use crate::venues::bsc_dex::pools::ekubo_v2::{IRouterV2, OrderKey};
use alloy::rpc::types::Log;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, address, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::HashMap;

fn json(raw: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(raw)).unwrap()
}
fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn official_router_wire_full_debits_and_recovery_match_wallet_replay() {
    let source = json(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ekubo_v2_ethereum_activity.json.gz"
    ));
    let replay = json(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ekubo_v2_ethereum_wallets.json.gz"
    ));
    assert_eq!(source["selector"], replay["selector"]);
    assert_eq!(source["state_root"], replay["state_root"]);
    let core = address(&source["core"]["address"]);
    let router = address(&source["router"]["address"]);
    let wrapped = address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
    let markets = source["markets"].as_array().unwrap();
    let deadline = U256::from(source["window"][1]["time"].as_u64().unwrap() + 600);
    let mut successes = HashMap::new();
    let mut rejects = 0;
    for trial in replay["cases"].as_array().unwrap() {
        let market = markets.iter().find(|m| m["id"] == trial["id"]).unwrap();
        let side = trial["side"].as_u64().unwrap() as usize;
        let k = &market["key"];
        let key = OrderKey {
            token0: address(&k["token0"]),
            token1: address(&k["token1"]),
            config: k["config"].as_str().unwrap().parse().unwrap(),
            side: side == 1,
            amount: trial["amount"].as_str().unwrap().parse().unwrap(),
            price_limit: market["price_limits"][side]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        };
        let owner = address(&trial["sender"]);
        let amount = number(&trial["amount"]);
        let out = number(&trial["out"]);
        for (name, minimum) in [("exact", out), ("above", out + U256::ONE)] {
            let row = &trial[name];
            let tx = &row["tx"];
            let result = &row["result"];
            let data: Bytes = serde_json::from_value(tx["data"].clone()).unwrap();
            assert_eq!(tx["from"], trial["sender"]);
            assert_eq!(address(&tx["to"]), router);
            assert_eq!(
                serde_json::from_value::<U256>(tx["value"].clone()).unwrap(),
                key.value()
            );
            assert_eq!(key.calldata(minimum, deadline), data);
            assert_eq!(
                IRouterV2::multicallCall::abi_decode_validate(&data)
                    .unwrap()
                    .data
                    .len(),
                4
            );
            let b = &row["balances"];
            let valid = trial["amount"] == trial["used"] && name == "exact";
            assert_eq!(result["status"] == "0x1", valid);
            assert_eq!(
                number(&b[0]) - number(&b[2]),
                if valid { amount } else { U256::ZERO }
            );
            assert_eq!(
                number(&b[3]) - number(&b[1]),
                if valid { out } else { U256::ZERO }
            );
            if !valid {
                assert!(result["logs"].as_array().unwrap().is_empty());
                rejects += 1;
                continue;
            }
            *successes
                .entry((market["id"].as_str().unwrap(), side))
                .or_insert(0) += 1;
            if !key.token0.is_zero() {
                continue;
            }
            let proof = Proof {
                core,
                router,
                payer: owner,
                wrapped,
                key,
                minimum,
                deadline,
                data_hash: keccak256(&data),
            };
            let recovered: Proof =
                serde_json::from_value(serde_json::to_value(proof).unwrap()).unwrap();
            assert_eq!(recovered.data(), data);
            let (input, output) = if side == 1 {
                (key.token1, wrapped)
            } else {
                (wrapped, key.token1)
            };
            let mut logs: Vec<Log> = result["logs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| Log {
                    inner: serde_json::from_value(v.clone()).unwrap(),
                    ..Default::default()
                })
                .collect();
            assert_eq!(
                recovered.amounts(&logs, owner, input, output),
                Some((amount, out))
            );
            assert!(
                recovered
                    .amounts(&logs, Address::ZERO, input, output)
                    .is_none()
            );
            let mut foreign = recovered.clone();
            foreign.router = Address::ZERO;
            assert!(foreign.amounts(&logs, owner, input, output).is_none());
            let event = logs
                .iter()
                .find(|v| v.address() == core && v.topics().is_empty())
                .unwrap()
                .clone();
            logs.push(event);
            assert!(recovered.amounts(&logs, owner, input, output).is_none());
        }
    }
    assert_eq!(successes.len(), 10);
    assert!(successes.values().all(|n| *n == 2));
    assert_eq!(rejects, 24);
}
