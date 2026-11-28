use super::receipt::{IWallet, Saved};
use crate::venues::bsc_dex::exec::signed_call::Erc20Call;
use alloy::{rpc::types::Log, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn ethereum_holder_module_binds_the_recorded_payment_and_both_wallet_deltas() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/zeroex_ethereum_wallet.json.gz"
        )[..],
    ))
    .unwrap();
    let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let amount = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let module = address(&archive["module"]);
    let payer = address(&archive["payer"]);
    let card: Value = serde_json::from_str(include_str!(
        "../../../../ui/src/assets/zeroex-ethereum-create.json"
    ))
    .unwrap();
    let runtime = archive["runtime"]
        .as_str()
        .unwrap()
        .parse::<Bytes>()
        .unwrap();
    assert_eq!(card["runtimeKeccak"], format!("{:#x}", keccak256(runtime)));
    let rows = archive["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    let mut measured = 0;
    for case in rows {
        let logs: Vec<Log> = serde_json::from_value(case["logs"].clone()).unwrap();
        let (debit, credit) = (amount(&case["debit"]), amount(&case["credit"]));
        let (input, output) = (address(&case["input"]), address(&case["output"]));
        let original = case["original"].as_str().unwrap().parse::<Bytes>().unwrap();
        let expiry = case["expiry"].as_u64().unwrap();
        let data = IWallet::settleCall {
            input,
            output,
            amount: debit,
            minimum: credit,
            expiry: U256::from(expiry),
            original: original.clone(),
        }
        .abi_encode();
        let mut proof = Saved::new(
            Erc20Call {
                chain: 1,
                payer,
                module,
                calldata: keccak256(&data),
            },
            keccak256(original),
            address(&archive["settler"]),
            (input, output),
            debit,
            credit,
            expiry,
        );
        let before = case["before"].as_array().unwrap();
        let after = case["after"].as_array().unwrap();
        assert_eq!(amount(&before[0]) - amount(&after[0]), debit);
        assert_eq!(amount(&after[1]) - amount(&before[1]), credit);
        if case["mode"] == "direct" {
            assert_eq!(proof.amounts(&logs), None);
            continue;
        }
        measured += 1;
        assert_eq!(proof.amounts(&logs), Some((debit, credit)));
        assert_eq!(case["negative"].as_array().unwrap().len(), 4);
        assert!(case["repeat"]["message"].is_string());
        let mut extra = logs.clone();
        extra.push(logs.last().unwrap().clone());
        assert_eq!(proof.amounts(&extra), None);
        assert_eq!(proof.amounts(&logs[..logs.len() - 1]), None);
        // При лучшей фактической выплате в план попадает гарантированный объём.
        let mut conservative = proof.clone();
        conservative.minimum -= U256::ONE;
        let t = super::routing::Ticket {
            proof: conservative,
            data: data.clone().into(),
            gas: case["gas"].as_u64().unwrap(),
            generation: 0,
            di: 18,
            dout: 6,
        };
        assert_eq!(t.units(true).0, credit - U256::ONE);
        assert_eq!(t.units(false).1, credit - U256::ONE);
        assert_eq!(t.proof.amounts(&logs), Some((debit, credit)));
        proof.minimum += U256::ONE;
        assert_eq!(proof.amounts(&logs), None);
        proof.minimum -= U256::ONE;
        proof.call.payer = address(&archive["maker"]);
        assert_eq!(proof.amounts(&logs), None);
    }
    assert_eq!(measured, 4);
}
