//! Точные расходы ETH и ERC20 MarSwap по исходным операциям архива. Контроль Base отделён от реальных рынков Ethereum.
use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;
fn fixture(b: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(b)).unwrap()
}
fn addr(x: &Value) -> Address {
    x.as_str().unwrap().parse().unwrap()
}
fn n(x: &Value) -> U256 {
    U256::from_str_radix(x.to_string().trim_matches('"'), 10).unwrap()
}
#[test]
fn two_ethereum_markets_and_all_flat_fills_use_the_signed_value() {
    use crate::venues::bsc_dex::node::abi_dex::IV2Router;
    use alloy::sol_types::SolCall;
    let profiles: Value =
        serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
    let profile = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "marswap-ethereum")
        .unwrap();
    let d = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/marswap-ethereum.json.gz"
    ));
    let mut rows = 0;
    let mut pools = 0;
    for p in d["markets"].as_array().unwrap() {
        let Some(cases) = p["settlements"]["cases"].as_array() else {
            continue;
        };
        pools += 1;
        for c in cases.iter().filter(|c| c["mode"] == "flat") {
            for side in ["buy", "sell"] {
                let v = &c[side];
                let (tx, result) = v["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(v["results"].as_array().unwrap())
                    .rev()
                    .find(|(tx, _)| {
                        tx["data"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("0x5c11d795"))
                    })
                    .unwrap();
                let bytes = tx["data"].as_str().unwrap().parse::<Bytes>().unwrap();
                let s=IV2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode_validate(&bytes).unwrap();
                let payer = addr(&tx["from"]);
                let paid = U256::from_str_radix(
                    tx["value"].as_str().unwrap().trim_start_matches("0x"),
                    16,
                )
                .unwrap();
                let proof = Proof {
                    market: Terms {
                        pair: addr(&p["pool"]),
                        router: addr(&tx["to"]),
                        wei: paid,
                        router_hash: profile["settings"]["dex.router_runtime_hash"]
                            .as_str()
                            .unwrap()
                            .parse()
                            .unwrap(),
                        pair_hash: p["runtime_hash"].as_str().unwrap().parse().unwrap(),
                    },
                    payer,
                    input: s.path[0],
                    output: s.path[1],
                    amount: s.amountIn,
                    minimum: s.amountOutMin,
                    deadline: s.deadline,
                };
                let logs: Vec<SimLog> = serde_json::from_value(result["logs"].clone()).unwrap();
                assert_eq!(proof.data(), bytes);
                assert_eq!(paid, n(&v["msg_value"]));
                assert_eq!(paid, n(&d["terms"][3]));
                assert_eq!(proof.measured(&logs), Some((n(&v["input"]), n(&v["net"]))));
                let mut other = proof.clone();
                other.market.pair = Address::with_last_byte(3);
                assert!(other.measured(&logs).is_none());
                other = proof;
                other.minimum = n(&v["net"]) + U256::ONE;
                assert!(other.measured(&logs).is_none());
                rows += 1;
            }
        }
    }
    assert_eq!((pools, rows), (2, 8));
}

#[test]
fn zero_factory_flat_keeps_four_variable_v5_fills() {
    let d = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/marswap_base_simulated.json.gz"
    ));
    let z = &d["synthetic_zero_flat"];
    assert_eq!(z["original_count"], 0);
    assert_eq!(z["fee"], 20);
    let mut counts = [0; 3];
    for c in z["cases"].as_array().unwrap() {
        match c["mode"].as_str().unwrap() {
            "v5" => {
                assert!(n(&c["output"]) > U256::ZERO);
                assert_eq!(n(&c["fee_value_wei"]), U256::ZERO);
                counts[0] += 1
            }
            m @ ("router" | "v5_no_percent") => {
                assert!(c["revert"]["message"].as_str().unwrap().contains("MSWAP"));
                counts[if m == "router" { 1 } else { 2 }] += 1
            }
            _ => panic!("неизвестный режим архива"),
        }
    }
    assert_eq!(counts, [4, 4, 4]);
}

#[test]
fn first_output_atom_can_survive_only_in_the_fixed_branch() {
    use crate::venues::bsc_dex::math::v2::{amount_in, amount_out};
    let d = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/marswap-ethereum.json.gz"
    ));
    let rs = &d["markets"][0]["reserves"];
    let (rin, rout) = (n(&rs[1]), n(&rs[0])); // ERC20 → WETH
    let den = U256::from(10_000);
    let keep = den - n(&d["terms"][1]) - n(&d["terms"][2]);
    let x = amount_in(U256::ONE, rin, rout, (den, den)).unwrap();
    assert_eq!(amount_out(x, rin, rout, (keep, den)), Some(U256::ZERO));
    assert_eq!(amount_out(x, rin, rout, (den, den)), Some(U256::ONE));
    assert!(n(&d["terms"][3]) > rout);
}
