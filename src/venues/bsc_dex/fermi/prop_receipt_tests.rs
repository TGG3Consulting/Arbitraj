//! Архивные выплаты Kaliber и Taurus. Восстановление сверяет события, подпись и пределы поручения.
use super::{fill, send};
use crate::venues::bsc_dex::{
    exec::{carbon_binding::Code, prop_wallet},
    fermi::slot::{Proof, executeAtSlotCall},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, I256, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::{Value, json};

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn num(v: &Value) -> U256 {
    U256::from_str_radix(
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
            .as_str(),
        10,
    )
    .unwrap()
}

#[tokio::test]
async fn both_builders_recover_the_complete_wallet_payout_from_the_original_slot() {
    let file = |bytes: &[u8]| -> Value { serde_json::from_reader(GzDecoder::new(bytes)).unwrap() };
    let frames: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/prop_ethereum_builder_frames.json"
    ))
    .unwrap();
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../../ui/src/assets/prop-slot-v2-ethereum-previous.json"
    ))
    .unwrap();
    let venues = [
        file(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/kaliber_ethereum_bound_v2.json.gz"
        )),
        file(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/taurus_ethereum_bound_v2.json.gz"
        )),
    ];
    let roles = file(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/kaliber_ethereum_wallets.json.gz"
    ));
    let payer = addr(&roles["owner"]);
    let executor = addr(&roles["module"]);
    let mut counts = Vec::new();
    for venue in venues {
        for b in venue["builders"].as_array().unwrap() {
            let builder = b["builder"].as_str().unwrap();
            let plan = &manifest["markets"][b["market"].as_str().unwrap()];
            let router = addr(&plan["addresses"][0]);
            let runtime: B256 = b["runtime"].as_str().unwrap().parse().unwrap();
            assert_eq!(plan["runtimeKeccak"], b["runtime"]);
            let mut paid = 0usize;
            for row in b["rows"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["withFrame"]["status"] == "paid")
            {
                let success = &row["withFrame"];
                let (spent, received) = (num(&success["paid"]), num(&success["net"]));
                let timestamp = b["timestamp"].as_u64().unwrap();
                let (input, output) = (addr(&row["input"]), addr(&row["output"]));
                let leg = keccak256(format!(
                    "{builder}{}{}",
                    row["input"].as_str().unwrap(),
                    row["output"].as_str().unwrap()
                ));
                let mut proof = Proof {
                    builder: builder.into(),
                    parent: b["parent"]["blockHash"].as_str().unwrap().parse().unwrap(),
                    target: frames["number"].as_u64().unwrap(),
                    timestamp,
                    executor,
                    payer,
                    router: Some(router),
                    input,
                    output,
                    amount: I256::try_from(spent).unwrap(),
                    limit: U256::ONE,
                    leg,
                    prop: Some(Box::new(prop_wallet::Proof {
                        terms: prop_wallet::Terms {
                            module_hash: Some(runtime),
                            target: executor,
                            fallback_allowed: false,
                            router,
                            venue: addr(&plan["addresses"][1]),
                            vault: addr(&plan["addresses"][2]),
                            fallback: addr(&roles["rows"][0]["fallback"]),
                            // Привязка прокси к блоку проверяется отдельно от денежных событий.
                            code: Code {
                                shell: plan["shells"][0].as_str().unwrap().parse().unwrap(),
                                implementation: Address::ZERO,
                                runtime: B256::ZERO,
                            },
                            venue_code: None,
                            vault_code: None,
                            via: None,
                        },
                        payer,
                        input,
                        output,
                        amount: spent,
                        minimum: U256::ONE,
                        deadline: U256::from(timestamp),
                    })),
                };
                let r:send::receipt::Receipt=serde_json::from_value(json!({"status":"0x1","blockNumber":row["logs"][0]["blockNumber"],"gasUsed":format!("0x{:x}",success["gas"].as_u64().unwrap()),"effectiveGasPrice":"0x0","logs":row["logs"]})).unwrap();
                let order = executeAtSlotCall::abi_decode_validate(&proof.calldata())
                    .unwrap()
                    .order;
                assert_eq!(
                    (order.leg, order.parent, order.height, order.timestamp),
                    (leg, proof.parent, proof.target, timestamp)
                );
                assert_eq!(
                    (order.tokenIn, order.tokenOut, order.amount, order.minimum),
                    (input, output, spent, U256::ONE)
                );
                assert_eq!(fill(&proof, &r, router), Some((spent, received)));
                // При покупке за заданный расход подписана минимальная выплата в один атом.
                proof.amount = -I256::ONE;
                proof.limit = spent;
                assert_eq!(fill(&proof, &r, router), Some((spent, received)));
                if output == alloy_primitives::address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") {
                    super::prop_fact_tests::checked_buy(&proof, &r, router, spent, received);
                }
                if builder == "Titan"
                    && input == alloy_primitives::address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
                    && output == alloy_primitives::address!("dac17f958d2ee523a2206206994597c13d831ec7")
                {
                    super::prop_fact_tests::through_core(&proof, &r, router, &row["logs"][0]).await;
                }
                proof.limit = spent - U256::ONE;
                assert_eq!(fill(&proof, &r, router), None);
                proof.limit = spent;
                let mut duplicate = r.clone();
                duplicate.logs.extend(r.logs.clone());
                assert_eq!(fill(&proof, &duplicate, router), None);
                // Удаляем по одному переводу, включая выплаты через кассу и через рынок.
                for i in 0..r.logs.len() {
                    if r.logs[i].address() != output
                        || r.logs[i].topic0()
                            != Some(&keccak256("Transfer(address,address,uint256)"))
                    {
                        continue;
                    }
                    let mut missing = r.clone();
                    missing.logs.remove(i);
                    assert_eq!(fill(&proof, &missing, router), None);
                }
                let mut foreign = proof.clone();
                foreign.leg = B256::ZERO;
                assert_eq!(fill(&foreign, &r, router), None);
                assert_eq!(fill(&proof, &r, executor), None);
                assert_eq!(row["reusedLeg"]["status"], "reverted");
                paid += 1;
            }
            counts.push(paid);
        }
    }
    assert_eq!(counts, [4, 6, 6]);
}
