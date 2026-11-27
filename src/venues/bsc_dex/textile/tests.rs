use super::{receipt::Saved, wire::*};
use crate::venues::bsc_dex::exec::signed_call::Erc20Call;
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolValue},
};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use chrono::DateTime;
use serde_json::{Value, json};

#[test]
fn single_and_batch_confirm_terminal_cash_on_base_and_ethereum() {
    let files: &[&[u8]] = &[
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/textile_base_wallets.json.gz"
        ),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/textile_ethereum_wallets.json.gz"
        ),
    ];
    for bytes in files {
        let doc: Value = serde_json::from_reader(flate2::read::GzDecoder::new(*bytes)).unwrap();
        let chain = doc["chain"].as_u64().unwrap();
        let reactor = addr(&doc["roles"]["reactor"]["address"]).unwrap();
        let historic: Value =
            serde_json::from_reader(flate2::read::GzDecoder::new(
                &include_bytes!(
                    "../../../../tests/data/first_run_dex_2026-11-26/textile_fx.json.gz"
                )[..],
            ))
            .unwrap();
        let collector = addr(
            &historic["chains"][if chain == 1 { "ethereum" } else { "base" }]["fee_recipient"][0],
        )
        .unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        for row in cases {
            let owner = addr(&row["payer"]).unwrap();
            let actor = addr(&row["taker"]).unwrap();
            let input = addr(&row["input"]).unwrap();
            let output = addr(&row["output"]).unwrap();
            let action: Bytes = row["calldata"].as_str().unwrap().parse().unwrap();
            let raw = if action.starts_with(&IReactor::executeCall::SELECTOR) {
                vec![
                    IReactor::executeCall::abi_decode_validate(&action)
                        .unwrap()
                        .order,
                ]
            } else {
                IReactor::executeBatchCall::abi_decode_validate(&action)
                    .unwrap()
                    .orders
            };
            let list:Vec<_>=raw.iter().map(|s|{
                let x=RawOrder::abi_decode_validate(&s.order).unwrap();
                let p=x.outputs[0].amount;
                let fee=p*U256::from(2)/U256::from(10000);
                json!({"maker":x.info.swapper,"encodedOrder":s.order,"signature":s.sig,"feeAmount":fee.to_string(),"sellAmount":(p+fee).to_string(),"buyAmount":x.input.amount.to_string()})
            }).collect();
            let expiry = row["expiry"].as_u64().unwrap();
            let end = DateTime::from_timestamp(expiry.try_into().unwrap(), 0)
                .unwrap()
                .to_rfc3339();
            let mut v = json!({"data":{"status":"quoted","quote":{"reactor":reactor,"spender":reactor,"taker":actor,"sellAmount":row["debit"],"takerPays":row["debit"],"buyAmount":row["credit"],"feeAmount":row["fee"],"expiresAt":end,"orderDeadline":end,"orders":list},
                "transactions":{"swap":{"to":reactor,"data":action,"value":"0","chainId":chain}}}});
            let demand = Demand {
                chain,
                reactor,
                taker: actor,
                input,
                output,
                amount: uint(&row["debit"]).unwrap(),
                exact_out: false,
                bps: U256::from(2),
            };
            let f = immediate(&v, &demand, expiry - 1).unwrap();
            assert_eq!(f.slices.len(), row["orders"].as_u64().unwrap() as usize);
            let target = if actor == owner { reactor } else { actor };
            let proof = Saved {
                call: Erc20Call {
                    chain,
                    payer: owner,
                    module: target,
                    calldata: keccak256(&action),
                },
                reactor,
                collector,
                action: keccak256(&action),
                input,
                output,
                debit: f.debit,
                minimum: f.credit,
                expiry,
                slices: f.slices,
            };
            let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
            assert_eq!(proof.amounts(&logs), Some((f.debit, f.credit)));
            assert!(
                immediate(
                    &v,
                    &Demand {
                        taker: Address::ZERO,
                        ..demand
                    },
                    expiry - 1
                )
                .is_none()
            );
            assert!(immediate(&v, &demand, expiry).is_none());
            v["data"]["quote"]["feeAmount"] = json!("0");
            assert!(immediate(&v, &demand, expiry - 1).is_none());
            v["data"]["quote"]["feeAmount"] = row["fee"].clone();
            v["data"]["route"] = json!("gmx-recall-v1");
            assert!(immediate(&v, &demand, expiry - 1).is_none());
        }
    }
}
