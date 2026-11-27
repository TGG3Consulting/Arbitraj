//! Оригинальная оплата Router по архивам Base и Ethereum переживает сохранение поручения.
use super::{
    super::registered_v3_wallet::{Market, Proof},
    Router,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, aliases::U24};
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn signed_market_matches_multicall_and_unique_pool_swap_on_both_chains() {
    let archives: [&[u8]; 2] = [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/maverick_v2_base_wallets.json.gz"
        ),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/maverick_v2_ethereum_wallets.json.gz"
        ),
    ];
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    for bytes in archives {
        let snap: Value = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
        let mut measured = 0;
        for row in snap["samples"].as_array().unwrap() {
            let Some(cases) = row["cases"].as_array() else {
                continue;
            };
            for case in cases.iter().filter(|c| c["mode"] == "router") {
                for side in ["buy", "sell"] {
                    let leg = &case[side];
                    let exact = &leg["exact"];
                    let data: Bytes = exact["calldata"].as_str().unwrap().parse().unwrap();
                    let multi = Router::multicallCall::abi_decode_validate(&data).unwrap();
                    let deadline = Router::checkDeadlineCall::abi_decode_validate(&multi.calls[0])
                        .unwrap()
                        .deadline;
                    let budget = exact["spent"].as_str().unwrap().parse::<U256>().unwrap();
                    let minimum = exact["received"].as_str().unwrap().parse::<U256>().unwrap();
                    let proof = Proof {
                        market: Market {
                            pool: addr(&row["pool"]),
                            fee: U24::ZERO,
                            algebra_deployer: None,
                            fee_event: false,
                            wallet_module: false,
                            slipstream: None,
                            maverick: true,
                        },
                        target: addr(&snap["roles"]["router"]["address"]),
                        payer: addr(&exact["payer"]),
                        input: addr(&leg["input"]),
                        output: addr(&leg["output"]),
                        budget,
                        minimum,
                        deadline,
                    };
                    let logs = serde_json::from_value::<
                        Vec<crate::venues::bsc_dex::check::simulate::SimLog>,
                    >(exact["logs"].clone())
                    .unwrap();
                    let mut restored: Proof =
                        serde_json::from_value(serde_json::to_value(proof).unwrap()).unwrap();
                    assert_eq!(restored.data(), data);
                    assert_eq!(restored.measured(&logs), Some((budget, minimum)));
                    restored.minimum += U256::ONE;
                    assert_eq!(restored.measured(&logs), None);
                    restored.minimum = minimum;
                    restored.target = Address::ZERO;
                    assert_eq!(restored.measured(&logs), None);
                    assert_eq!(leg["above"]["status"], "0x0");
                    assert_eq!(leg["expired"]["status"], "0x0");
                    measured += 1;
                }
            }
        }
        assert_eq!(measured, 8);
    }
}
