//! Сверяет исходную формулу и чистый платёж с архивными действиями каждого из пяти физических пулов.
use crate::core::types::EmpireState;
use alloy_primitives::{B256, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;

fn raw(x: &Value) -> U256 {
    x.as_str().unwrap().parse().unwrap()
}
fn words(log: &Value) -> Vec<U256> {
    let h = log["data"].as_str().unwrap().strip_prefix("0x").unwrap();
    (0..h.len())
        .step_by(64)
        .map(|i| U256::from_str_radix(&h[i..i + 64], 16).unwrap())
        .collect()
}
fn logs<'a>(j: &'a Value, topic: B256) -> impl Iterator<Item = &'a Value> {
    j["logs"].as_array().unwrap().iter().filter(move |l| {
        l["topics"][0].as_str().and_then(|h| h.parse::<B256>().ok()) == Some(topic)
    })
}

#[test]
fn all_module_receipts_preserve_gross_credit_and_wallet_guards() {
    let d: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/empire_ethereum_protected.json.gz"
        )[..],
    ))
    .unwrap();
    let filled = keccak256(
        "Filled(address,address,address,address,address,uint256,uint256,uint256,uint256)",
    );
    let swap = keccak256("Swap(address,uint256,uint256,uint256,uint256,address)");
    let mut verified = [0, 0];
    for row in d["pools"].as_array().unwrap() {
        for sample in row["samples"].as_array().unwrap() {
            let result = &sample["result"];
            let Some(exact) = result.get("exact") else {
                assert_eq!(result["loose"]["status"], "0x0");
                continue;
            };
            let acts: Vec<_> = logs(exact, filled).collect();
            assert_eq!(acts.len(), 1);
            let [_, _, debit, credit, gross, received]: [U256; 6] =
                words(acts[0]).try_into().unwrap();
            let before = &exact["before"];
            let enters_weth = sample["side"] == "buy";
            let zero_in =
                (row["tokens"][0] == "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") == enters_weth;
            let i = usize::from(!zero_in);
            let mut balances = [U256::ZERO; 2];
            balances[i] = raw(&before["physical"][0]);
            balances[1 - i] = raw(&before["physical"][1]);
            let mut debt = [U256::ZERO; 2];
            for (side, value) in debt.iter_mut().enumerate() {
                if row["sweepable"] == row["tokens"][side] {
                    *value = raw(&before["debt"])
                }
            }
            let s = EmpireState {
                reserves: [raw(&before["reserves"][0]), raw(&before["reserves"][1])],
                balances,
                debt,
                debt_aware: true,
                exact_tokens: false,
            };
            let moved = raw(&exact["after"]["physical"][0]) - balances[i];
            let (model, _) = s.paid(zero_in, moved.min(debit)).unwrap();
            assert_eq!(s.credit(zero_in, moved.min(debit)), Some(credit));
            assert_eq!(model.amount_out, gross);
            let events: Vec<_> = logs(exact, swap)
                .filter(|l| l["address"] == row["pool"])
                .collect();
            assert_eq!(words(events.last().unwrap())[3 - i], gross);
            assert_eq!(raw(&exact["received"]), received);
            assert_eq!(raw(&exact["paid"]), raw(&sample["input"]));
            assert_eq!(result["above"]["status"], "0x0");
            assert_eq!(result["expired"]["status"], "0x0");
            verified[usize::from(sample["mode"] == "v5module")] += 1;
        }
    }
    assert_eq!(verified, [16, 16]);
}
