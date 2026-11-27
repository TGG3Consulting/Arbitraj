use super::*;
use crate::venues::bsc_dex::check::simulate::SimLog;
use serde_json::Value;

#[test]
fn original_output_funding_instruction_and_receipt_survive_recovery() {
    let raw = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/antfarm_output_ethereum.json.gz"
    );
    let data: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
    let module: Address = serde_json::from_value(data["module"].clone()).unwrap();
    assert_eq!(
        serde_json::from_value::<B256>(data["runtime_hash"].clone()).unwrap(),
        RUNTIME
    );
    for row in data["cases"].as_array().unwrap() {
        let calls = row["calls"].as_array().unwrap();
        let i = calls
            .iter()
            .position(|v| v["to"] == data["module"])
            .unwrap();
        let calldata = serde_json::from_value::<Bytes>(calls[i]["data"].clone()).unwrap();
        let payer = serde_json::from_value(calls[i]["from"].clone()).unwrap();
        let original = executeCall::abi_decode_validate(&calldata).unwrap().order;
        let t = Terms {
            module,
            input: original.input,
            output: original.output,
            tier: original.tier,
            funding: original.fundingTier,
            use_loan: original.useLoan,
            amount: original.amount,
            max_atf: original.maxAtf,
            max_funding: original.maxFunding,
            max_loan_fee: original.maxLoanFee,
        };
        let p = Proof {
            terms: t,
            payer,
            minimum: original.minCredit,
            deadline: original.deadline,
        };
        let mut saved: Proof = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        assert_eq!(saved.data(), calldata);
        let logs: Vec<SimLog> = serde_json::from_value(row["results"][i]["logs"].clone()).unwrap();
        let expected = (
            original.amount,
            row["credit"].as_str().unwrap().parse().unwrap(),
        );
        assert_eq!(saved.measured(&logs), Some(expected));
        assert_eq!(saved.terms.stock_approval().is_some(), !original.useLoan);
        let mut partial = logs.clone();
        let settled = logs.iter().find(|l| l.address == module).unwrap().clone();
        partial.push(settled);
        assert!(saved.measured(&partial).is_none());
        if !original.useLoan {
            let lost_refund: Vec<_> = logs
                .iter()
                .filter(|l| {
                    !(l.address == antfarm_book::FEE_TOKEN
                        && l.topics.get(2) == Some(&payer.into_word()))
                })
                .cloned()
                .collect();
            assert!(saved.measured(&lost_refund).is_none());
        }
        saved.minimum = expected.1 + U256::ONE;
        assert!(saved.measured(&logs).is_none());
    }
}
