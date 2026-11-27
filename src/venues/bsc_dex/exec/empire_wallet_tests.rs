use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn archived_module_receipts_bind_the_signed_pool_and_real_wallet_including_reflections() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/empire_ethereum_protected.json.gz"
        )[..],
    ))
    .unwrap();
    let mut checked = 0;
    for row in archive["pools"].as_array().unwrap() {
        for sample in row["samples"].as_array().unwrap() {
            let exact = &sample["result"]["exact"];
            if sample["mode"] != "module" || exact.is_null() {
                continue;
            }
            let logs: Vec<Log> = serde_json::from_value(exact["logs"].clone()).unwrap();
            let payment = logs
                .iter()
                .find(|l| l.topic0() == Some(&Filled::SIGNATURE_HASH))
                .unwrap();
            let f = Filled::decode_log_data(&payment.inner.data).unwrap();
            let amt = |key: &str| exact[key].as_str().unwrap().parse::<U256>().unwrap();
            let data: Bytes = exact["calldata"].as_str().unwrap().parse().unwrap();
            let proof = Proof {
                module: payment.address(),
                pool: row["pool"].as_str().unwrap().parse().unwrap(),
                payer: archive["owner"].as_str().unwrap().parse().unwrap(),
                input: f.input,
                output: f.output,
                amount: amt("paid"),
                minimum: amt("received"),
                deadline: U256::from_be_slice(&data[data.len() - 32..]),
            };
            assert_eq!(proof.data(), data);
            let amounts =
                |p: &Proof, ls: &[Log]| p.amounts(ls, proof.payer, proof.input, proof.output);
            assert_eq!(amounts(&proof, &logs), Some((proof.amount, proof.minimum)));
            let mut wrong = proof.clone();
            wrong.minimum += U256::ONE;
            assert_eq!(amounts(&wrong, &logs), None);
            let mut twice = logs.clone();
            twice.push(payment.clone());
            assert_eq!(amounts(&proof, &twice), None);
            checked += 1;
        }
    }
    assert_eq!(checked, 16);
}
