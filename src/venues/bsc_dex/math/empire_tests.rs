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
fn logs(j: &Value, topic: B256) -> impl Iterator<Item = &Value> {
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

#[test]
fn compiled_wallet_fee_matches_the_accounting_units() {
    let contract = include_str!("../../../../contracts/EmpireWallet.sol");
    for (name, value) in [
        ("FEE_UNITS", super::empire::FEE_UNITS),
        ("FEE_SCALE", super::empire::FEE_SCALE),
    ] {
        assert!(contract.contains(&format!("uint256 private constant {name}={value};")));
    }
}

#[test]
fn measured_boundary_keeps_swept_input_distinct_from_actual_deposit() {
    let raw_fixture = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/empire_ethereum_wallets.json.gz"
    );
    let module_fixture = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/empire_ethereum_protected.json.gz"
    );
    let mut checked = 0;
    for bytes in [&raw_fixture[..], &module_fixture[..]] {
        let archive: Value = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
        for pool in archive["pools"].as_array().unwrap() {
            let initial = &pool["initial"];
            let zero = pool["tokens"][0] == "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2";
            let i = usize::from(!zero);
            for sample in pool["debt_boundary"].as_array().unwrap() {
                let mut debt = [U256::ZERO; 2];
                debt[i] = raw(&initial["debt"]);
                let amount = raw(&sample["input"]);
                let res = &sample["result"];
                let aware = matches!(
                    sample["mode"].as_str(),
                    Some("module" | "v5module" | "nominal")
                );
                let state = EmpireState {
                    debt,
                    debt_aware: aware,
                    exact_tokens: false,
                    balances: [raw(&initial["physical"][0]), raw(&initial["physical"][1])],
                    reserves: [raw(&initial["reserves"][0]), raw(&initial["reserves"][1])],
                };
                if !aware && amount < debt[i] {
                    assert!(state.paid(zero, amount).is_none());
                    assert_eq!(res["loose"]["status"], "0x0");
                } else {
                    let (fill, _) = state.paid(zero, amount).unwrap();
                    let swap = keccak256("Swap(address,uint256,uint256,uint256,uint256,address)");
                    let mut physical =
                        logs(&res["exact"], swap).filter(|l| l["address"] == pool["pool"]);
                    assert_eq!(words(physical.next().unwrap())[3 - i], fill.amount_out);
                    assert!(physical.next().is_none());
                    assert_eq!(raw(&res["exact"]["paid"]), amount);
                    assert_eq!(
                        state.credit(zero, amount),
                        Some(if aware { amount } else { amount - debt[i] })
                    );
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 20);
}
