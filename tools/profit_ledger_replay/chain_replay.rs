
#[derive(Default, serde::Serialize)]
struct ChainStats {
    cases: usize,
    matched_evidenced_credit: usize,
    different_quantity: usize,
    no_unambiguous_payment: usize,
}

#[test]
fn all_observed_chain_deposits() {
    use alloy_primitives::{Address, B256, U256};
    use arb::venues::bsc_dex::wallet::transfer_status::audit_receipt_credit;
    use std::str::FromStr;
    let input: serde_json::Value = serde_json::from_slice(&fs::read(env::var("PRIVATE_LEDGER_INPUT").unwrap()).unwrap()).unwrap();
    let mut groups: BTreeMap<String, ChainStats> = BTreeMap::new();
    for row in input["receipt_cases"].as_array().unwrap() {
        let hash: B256 = row["receipt"]["transactionHash"].as_str().unwrap().parse().unwrap();
        let token: Address = row["token"].as_str().unwrap().parse().unwrap();
        let wallet: Address = row["wallet"].as_str().unwrap().parse().unwrap();
        let observed = U256::from_str(row["observed_atomic"].as_str().unwrap()).unwrap();
        let expected = row["expected_atomic"].as_str().map(|v| U256::from_str(v).unwrap());
        let result = audit_receipt_credit(row["receipt"].clone(), token, wallet, hash, expected);
        for label in ["all", row["kind"].as_str().unwrap()] {
            let s = groups.entry(label.to_string()).or_default();
            s.cases += 1;
            match result {
                Some(q) if q == observed => s.matched_evidenced_credit += 1,
                Some(_) => s.different_quantity += 1,
                None => s.no_unambiguous_payment += 1,
            }
        }
    }
    assert!(groups["all"].cases >= 3800, "incomplete primary receipt corpus");
    fs::write(env::var("CHAIN_RESULT").unwrap(), serde_json::to_vec_pretty(&groups).unwrap()).unwrap();
}
