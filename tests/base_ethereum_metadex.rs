//! Сверяет первую комиссию Aero MetaDEX, ответы Metaquoter и фактические балансы V5.
//! Данные снимают scripts/record_metadex_* у публичных узлов привязкой к каноническому блоку.
use alloy::sol_types::SolCall;
use alloy_primitives::{U256, hex, keccak256};
use arb::venues::bsc_dex::{node::abi_dex::IAeroMetaquoter, pools::aero_first_fee as fee};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn data() -> [Value; 2] {
    [
        include_str!("data/first_run_dex_2026-11-26/metadex_base.json"),
        include_str!("data/first_run_dex_2026-11-26/metadex_ethereum.json"),
    ]
    .map(|s| serde_json::from_str(s).unwrap())
}

fn n(v: &Value) -> u32 {
    v.as_u64().unwrap().try_into().unwrap()
}
fn raw(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}
fn obs(v: &Value) -> fee::Obs {
    fee::Obs {
        time: n(&v[0]),
        initialized: v[1].as_bool().unwrap(),
    }
}

#[test]
fn port_reproduces_first_fee_for_metaquoter_and_executor() {
    for j in data() {
        let ahead = 2 * n(&j["slot_seconds"]);
        for p in j["pools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p.get("head_fee").is_some())
        {
            let f = &p["head_fee"];
            let v = &f["config"];
            let config = fee::Config {
                base_fee: n(&v[0]),
                fee_cap: n(&v[1]),
                scaling: v[2].as_u64().unwrap(),
                initial_enabled: v[3].as_bool().unwrap(),
                initial_fee: n(&v[4]),
            };
            let defaults = fee::Defaults {
                scaling: U256::from(n(&f["defaultScalingFactor"])),
                fee_cap: U256::from(n(&f["defaultFeeCap"])),
                seconds_ago: n(&f["secondsAgo"]),
            };
            let head = fee::Head {
                time: raw(&j["timestamp"]).to::<u64>(),
                tick: f["tick"].as_i64().unwrap().try_into().unwrap(),
                cardinality: n(&f["cardinality"]).try_into().unwrap(),
                oldest: obs(&f["oldest"]),
                zero: obs(&f["zero"]),
                cums: serde_json::from_value(f["cums"].clone()).unwrap(),
            };
            assert_eq!(head.cums.len(), ahead as usize + 1);
            let ts = n(&f["tick_fee"]);
            let upper = fee::worst(&config, &defaults, ts, &head, ahead);
            for probe in f["probes"].as_array().unwrap() {
                let delta = n(&probe["seconds"]);
                assert!((1..=ahead).contains(&delta));
                let dyn_fee =
                    fee::dynamic_at(&head, &defaults, fee::scaling_of(&config, &defaults), delta);
                let (first, _) = fee::first(&config, &defaults, ts, dyn_fee);
                assert_eq!(probe["requests"][0]["from"], j["quoter"]);
                assert_eq!(probe["requests"][1]["from"], j["v5"]["executor"]);
                for result in probe["results"].as_array().unwrap() {
                    assert_eq!(result["status"], "0x1");
                    let actual = raw(&result["returnData"]);
                    assert_eq!(actual, first);
                    assert!(actual <= U256::from(upper));
                }
            }
        }
    }
}

#[test]
fn direct_quote_matches_v5_swap_event_and_wallet_with_exact_minimum() {
    let runtime = hex::decode(
        include_str!("../contracts/out/ArbExecutor.bin-runtime")
            .trim()
            .trim_start_matches("0x"),
    )
    .unwrap();
    let digest = hex::encode(Sha256::digest(runtime));
    let event = format!(
        "{:#x}",
        keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)")
    );
    for j in data() {
        assert_eq!(j["v5"]["runtime_sha256"], digest);
        let cases = j["v5"]["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        let mut coverage = std::collections::BTreeMap::new();
        for c in cases {
            let pool = c["pool"].as_str().unwrap();
            let forward = c["forward"].as_bool().unwrap();
            let input = U256::from_str_radix(c["amount"].as_str().unwrap(), 10).unwrap();
            coverage
                .entry((pool, forward))
                .or_insert_with(std::collections::BTreeSet::new)
                .insert(input);
            let r = &c["results"];
            let calls = &c["calldata"];
            let qtype = IAeroMetaquoter::quoteExactInputSingleV3Call::abi_decode(
                &hex::decode(calls["quote"]["data"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(qtype.amountIn, input);
            assert_eq!(format!("{:#x}", qtype.pool), pool);
            assert_eq!(calls["quote"]["to"], j["quoter"]);
            let quote = IAeroMetaquoter::quoteExactInputSingleV3Call::abi_decode_returns(
                &hex::decode(r["quote"]["returnData"].as_str().unwrap()).unwrap(),
            )
            .unwrap()
            .amountOut;
            assert_eq!(
                raw(&r["owner_out_after"]["returnData"])
                    - raw(&r["owner_out_before"]["returnData"]),
                quote
            );
            assert_eq!(
                raw(&r["owner_in_before"]["returnData"]) - raw(&r["owner_in_after"]["returnData"]),
                input
            );
            assert_eq!(r["execute"]["status"], "0x1");
            assert_eq!(raw(&r["execute"]["returnData"]), quote);
            assert_eq!(raw(&r["executor_input"]["returnData"]), U256::ZERO);
            assert_eq!(raw(&r["executor_output"]["returnData"]), U256::ZERO);
            let log = r["execute"]["logs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["address"] == pool && l["topics"][0] == event)
                .unwrap();
            let deltas = hex::decode(log["data"].as_str().unwrap()).unwrap();
            let a = U256::from_be_slice(&deltas[..32]);
            let b = U256::from_be_slice(&deltas[32..64]);
            let neg_out = U256::ZERO.wrapping_sub(quote);
            assert!((a == input && b == neg_out) || (b == input && a == neg_out));
            let original = hex::decode(calls["execute"]["data"].as_str().unwrap()).unwrap();
            let mut wrong = hex::decode(c["rejected_calldata"].as_str().unwrap()).unwrap();
            assert_eq!(U256::from_be_slice(&original[100..132]), quote);
            assert_eq!(U256::from_be_slice(&wrong[100..132]), quote + U256::ONE);
            wrong[100..132].copy_from_slice(&original[100..132]);
            assert_eq!(wrong, original);
            assert_eq!(c["rejected"]["status"], "0x0");
        }
        assert_eq!(coverage.len(), 4);
        assert!(coverage.values().all(|v| v.len() == 2));
    }
}
