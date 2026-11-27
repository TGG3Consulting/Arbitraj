//! Архивный прямой перевод WETH и B20. Проверяет идентичность рынка и восстановление поручения.
use super::{
    o1_wallet::{IWallet, Proof},
    signed_call,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{B256, Bytes, U256};
use serde_json::Value;

#[test]
fn restored_receipt_accepts_only_the_signed_physical_key() {
    let bytes = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/o1_wrapped_base_wallets.json.gz"
    );
    let archive: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
    let code: Bytes = archive["module"]["runtime"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let config: Value =
        serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
    let profile = config
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["chain_id"] == 8453 && r["id"] == "o1-launchpad-wrapped")
        .unwrap();
    assert!(profile["settings"].get("dex.router").is_none());
    let expected: B256 = archive["module"]["keccak"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(alloy_primitives::keccak256(code), expected);
    let allowed = profile["settings"]["dex.module_runtime_hashes"]
        .as_array()
        .unwrap();
    assert_eq!(
        allowed
            .iter()
            .filter(|value| value.as_str().unwrap().parse::<B256>().unwrap() == expected)
            .count(),
        1
    );
    let mut n = 0;
    for m in archive["markets"].as_array().unwrap() {
        let id = m["pool"].as_str().unwrap().parse().unwrap();
        for case in m["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["mode"] == "wrapped")
        {
            for side in ["buy", "sell"] {
                let leg = &case[side];
                let calls = leg["calls"].as_array().unwrap();
                let i = calls.len() - 3;
                let raw: Bytes = calls[i]["data"].as_str().unwrap().parse().unwrap();
                let call = IWallet::executeCall::abi_decode_validate(&raw).unwrap();
                assert_eq!(call.id, id);
                let payer = calls[i]["from"].as_str().unwrap().parse().unwrap();
                let module = calls[i]["to"].as_str().unwrap().parse().unwrap();
                let proof = Proof {
                    id,
                    module,
                    payer,
                    input: call.tokenIn,
                    output: call.tokenOut,
                    amount: call.qty,
                    minimum: call.minimum,
                    deadline: call.deadline,
                };
                let mut restored: Proof =
                    serde_json::from_value(serde_json::to_value(proof).unwrap()).unwrap();
                assert_eq!(restored.data(), raw);
                let rows =
                    serde_json::from_value::<Vec<_>>(leg["results"][i]["logs"].clone()).unwrap();
                let logs = signed_call::logs(&rows).unwrap();
                assert_eq!(
                    restored.amounts(&logs, payer, call.tokenIn, call.tokenOut),
                    Some((call.qty, call.minimum))
                );
                restored.id = B256::ZERO;
                assert_eq!(
                    restored.amounts(&logs, payer, call.tokenIn, call.tokenOut),
                    None
                );
                restored.id = id;
                restored.minimum = call.minimum + U256::from(1);
                assert_eq!(
                    restored.amounts(&logs, payer, call.tokenIn, call.tokenOut),
                    None
                );
                n += 1;
            }
        }
    }
    assert_eq!(n, 12);
}
