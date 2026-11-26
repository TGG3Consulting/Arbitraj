use alloy_primitives::{Address, Bytes, U256, keccak256};
use arb::venues::bsc_dex::{pools::metric_list, quote::virtual_quoter};
use serde_json::Value;

fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn raw(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn registered_metric_pools_fill_both_sides_for_two_senders() {
    let code = virtual_quoter::runtime().unwrap();
    let current = include_str!("../contracts/out/ArbExecutor.bin-runtime");
    let executor: Bytes = format!("0x{}", current.trim().trim_start_matches("0x"))
        .parse()
        .unwrap();
    for capture in [
        include_str!("data/first_run_dex_2026-11-26/metric_base.json"),
        include_str!("data/first_run_dex_2026-11-26/metric_ethereum.json"),
    ] {
        let j: Value = serde_json::from_str(capture).unwrap();
        let samples = j["executor_probes"].as_array().unwrap();
        assert_eq!(samples.len(), 2);
        assert_ne!(samples[0]["sender"], samples[1]["sender"]);
        for sample in samples {
            assert_eq!(sample["hash"], j["hash"]);
            assert_eq!(
                sample["virtual_quoter_keccak"],
                format!("{:#x}", keccak256(&code))
            );
            assert_eq!(
                sample["executor_runtime_keccak"],
                format!("{:#x}", keccak256(&executor))
            );
            let pools = sample["pools"].as_array().unwrap();
            assert!(pools.len() >= 3);
            for p in pools {
                let imm: Bytes = p["immutables"].as_str().unwrap().parse().unwrap();
                let (factory, t0, t1) = metric_list::tokens_of(&imm).unwrap();
                assert_eq!(
                    (factory, t0, t1),
                    (
                        address(&j["factory"]),
                        address(&p["tokens"][0]),
                        address(&p["tokens"][1])
                    )
                );
                let cases = p["cases"].as_array().unwrap();
                assert_eq!(cases.len(), 2);
                assert_ne!(cases[0]["input"], cases[1]["input"]);
                for c in cases {
                    assert!(raw(&c["bought"]) > U256::ZERO && raw(&c["sold"]) > U256::ZERO);
                    assert_eq!(
                        raw(&c["cash_before"]) - raw(&c["input"]) + raw(&c["sold"]),
                        raw(&c["cash_after"])
                    );
                    for field in [
                        "after_coin_owner",
                        "after_coin_executor",
                        "after_cash_executor",
                    ] {
                        assert_eq!(raw(&c[field]), U256::ZERO);
                    }
                    assert_eq!(c["min_out_plus_one_buy_reverted"], true);
                    assert_eq!(c["min_out_plus_one_sell_reverted"], true);
                }
            }
        }
    }
}
