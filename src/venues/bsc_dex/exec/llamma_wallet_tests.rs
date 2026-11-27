use super::*;
use crate::venues::bsc_dex::quote::v2_wallet_sim::Wire;
use flate2::read::GzDecoder;
use serde_json::Value;
#[test]
fn restore_all_funded_calls_and_reject_different_receiver_or_duplicate_paid() {
    let raw = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/llamma_ethereum_module.json.gz"
    );
    let j: Value = serde_json::from_reader(GzDecoder::new(&raw[..])).unwrap();
    let owner = j["owner"].as_str().unwrap().parse().unwrap();
    let target = j["module"].as_str().unwrap().parse().unwrap();
    let mut count = 0;
    for item in j["rows"].as_array().unwrap() {
        let bytes: Bytes = item["trade"]["data"].as_str().unwrap().parse().unwrap();
        let x = abi::IModule::executeCall::abi_decode(&bytes).unwrap().order;
        let by_target = item["mode"] == "target";
        assert_eq!(by_target, x.byTarget);
        let terms = Terms {
            index: x.index.to(),
            pool: x.pool,
            pool_hash: x.poolHash,
            target,
            module_hash: j["runtimes"][item["kind"].as_str().unwrap()]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            zero: if item["side"] == 0 { x.input } else { x.output },
            desired: by_target.then_some(x.minimum),
        };
        let order = Order {
            payer: owner,
            target,
            input: x.input,
            output: x.output,
            amount: x.maximum,
            minimum: x.minimum,
            wire: Wire::Llamma(terms),
        };
        let proof = terms.proof(&order, x.minimum, x.deadline);
        let recovered: Proof =
            serde_json::from_slice(&serde_json::to_vec(&proof).unwrap()).unwrap();
        assert_eq!(recovered.data(), bytes);
        let mut logs: Vec<SimLog> = serde_json::from_value(item["logs"].clone()).unwrap();
        let paid = (
            item["spent"].as_str().unwrap().parse().unwrap(),
            item["received"].as_str().unwrap().parse().unwrap(),
        );
        assert_eq!(recovered.measured(&logs), Some(paid));
        assert!(
            recovered
                .amounts(
                    &signed_call::logs(&logs).unwrap(),
                    target,
                    x.input,
                    x.output
                )
                .is_none()
        );
        let extra = logs
            .iter()
            .find(|l| {
                l.address == target && l.topics.first() == Some(&abi::IModule::Paid::SIGNATURE_HASH)
            })
            .unwrap()
            .clone();
        logs.push(extra);
        assert!(recovered.measured(&logs).is_none());
        count += 1;
    }
    assert_eq!(count, 44);
}
