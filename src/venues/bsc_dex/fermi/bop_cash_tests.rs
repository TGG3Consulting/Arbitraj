//! Реальные кассовые вызовы на двух кадрах Titan и BuilderNet, записанные в GitHub Actions.
use super::*;
use crate::venues::bsc_dex::fermi::{bop_cash::Order, bop_versions, slot};
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use std::str::FromStr;

#[test]
fn published_cash_and_direct_calls_recover_only_the_original_wallet_and_path() {
    let data = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/bopamm_ethereum_cash_wallet.json.gz"
    );
    let archive: Value = serde_json::from_reader(GzDecoder::new(data.as_slice())).unwrap();
    let mut counts = [0, 0];
    for frame in archive["frames"].as_array().unwrap() {
        let get = |v: &Value, key: &str| U256::from_str(v[key].as_str().unwrap()).unwrap();
        for row in frame["cases"].as_array().unwrap() {
            for (idx, name) in ["direct", "compound"].iter().enumerate() {
                let fill_data = &row[*name];
                if fill_data["outcome"] != "paid" {
                    continue;
                }
                counts[idx] += 1;
                let spent = get(fill_data, "spent");
                let net = get(fill_data, "received");
                let input: Address = row["input"].as_str().unwrap().parse().unwrap();
                let buy = input == USDT;
                let proof = slot::Proof {
                    builder: frame["builder"].as_str().unwrap().into(),
                    parent: frame["parent"].as_str().unwrap().parse().unwrap(),
                    target: frame["height"].as_u64().unwrap(),
                    timestamp: 1_606_824_023 + frame["slot"].as_u64().unwrap() * 12,
                    executor: fill_data["executor"].as_str().unwrap().parse().unwrap(),
                    router: Some(MARKET),
                    prop: None,
                    bop: Some(Order {
                        bridge: idx == 1,
                        paid: spent,
                        minimum: net,
                        runtime: bop_versions::CASH,
                    }),
                    payer: frame["holder"].as_str().unwrap().parse().unwrap(),
                    input,
                    output: row["output"].as_str().unwrap().parse().unwrap(),
                    amount: if buy {
                        -I256::from_raw(net)
                    } else {
                        I256::from_raw(spent)
                    },
                    limit: if buy { spent } else { net },
                    leg: fill_data["leg"].as_str().unwrap().parse().unwrap(),
                };
                assert_eq!(
                    format!("{:#x}", proof.calldata()),
                    fill_data["data"].as_str().unwrap()
                );
                assert_eq!(fill_data["duplicate_reverted"], true);
                let mut receipt:send::receipt::Receipt=serde_json::from_value(json!({"blockNumber":"0x1","status":"0x1","gasUsed":"0x1","effectiveGasPrice":"0x1","logs":[]})).unwrap();
                for log in fill_data["logs"].as_array().unwrap() {
                    receipt.logs.push(alloy::rpc::types::Log {
                        inner: serde_json::from_value(log.clone()).unwrap(),
                        ..Default::default()
                    });
                }
                assert_eq!(fill(&proof, &receipt, MARKET), Some((spent, net)));
                let mut wrong = proof.clone();
                wrong.leg = B256::ZERO;
                assert_eq!(fill(&wrong, &receipt, MARKET), None);
                if idx == 1 {
                    receipt.logs.retain(|l| l.address() != POOL);
                    assert_eq!(fill(&proof, &receipt, MARKET), None);
                }
            }
        }
    }
    assert_eq!(counts, [3, 12]);
}
