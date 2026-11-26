//! Проверяет исходные ответы eth_simulateV1, границы minOut и движение обоих токенов на новых сетях.
//! Вызывается тестом bsc_dex_executor_v5_live. Входы — два реальных no-hook USDT/USDC PoolKey на сеть.
use alloy_primitives::{U256, hex};
use serde_json::Value;

/// Первое 32-байтовое слово результата. Quoter V4 также возвращает оценку газа после суммы.
fn first_word(hexstr: &Value) -> U256 {
    let bytes = hex::decode(hexstr.as_str().unwrap()).unwrap();
    U256::from_be_slice(&bytes[..32])
}

/// Для каждой стороны calldata должна различаться ровно значением minOut на единицу.
fn boundary(bad: &Value, good: &Value, amount: U256) {
    assert_eq!(bad["from"], good["from"]);
    assert_eq!(bad["to"], good["to"]);
    let mut wrong = hex::decode(bad["input"].as_str().unwrap()).unwrap();
    let correct = hex::decode(good["input"].as_str().unwrap()).unwrap();
    let range = 4 + 3 * 32..4 + 4 * 32;
    assert_eq!(U256::from_be_slice(&correct[range.clone()]), amount);
    assert_eq!(
        U256::from_be_slice(&wrong[range.clone()]),
        amount + U256::from(1)
    );
    wrong[range.clone()].copy_from_slice(&correct[range]);
    assert_eq!(wrong, correct);
}

#[test]
fn both_networks_match_output_balances_and_enforce_exact_minimum() {
    for (raw, pinned) in [
        (
            include_str!("../data/executor_v5_base_2026-11-26/simulation_rpc.json"),
            include_str!("../data/first_run_dex_2026-11-26/base.json"),
        ),
        (
            include_str!("../data/executor_v5_ethereum_2026-11-26/simulation_rpc.json"),
            include_str!("../data/first_run_dex_2026-11-26/ethereum.json"),
        ),
    ] {
        let records: Vec<Value> = serde_json::from_str(raw).unwrap();
        let pin: Value = serde_json::from_str(pinned).unwrap();
        let mut found = 0;
        for row in records {
            let input = &row["params"][0]["blockStateCalls"][0];
            let asks = input["calls"].as_array().unwrap();
            if asks.len() != 14 {
                continue;
            }
            found += 1;
            assert_eq!(row["params"][1], pin["selector"]);
            let block = &row["result"][0];
            assert_eq!(block["parentHash"], pin["selector"]["blockHash"]);
            let replies = block["calls"].as_array().unwrap();
            assert_eq!(replies.len(), asks.len());
            for (i, r) in replies.iter().enumerate() {
                let rejected = i == 3 || i == 8;
                assert_eq!(r["status"], if rejected { "0x0" } else { "0x1" });
            }
            let value = |index: usize| first_word(&replies[index]["returnData"]);
            let (buy, sell) = (value(2), value(7));
            boundary(&asks[3], &asks[4], buy);
            boundary(&asks[8], &asks[9], sell);
            let amount = hex::decode(asks[4]["input"].as_str().unwrap()).unwrap();
            let initial_in = U256::from_be_slice(&amount[4 + 32..4 + 64]);
            assert_eq!(value(4), buy);
            assert_eq!(value(9), sell);
            assert_eq!(value(0) - value(5), initial_in);
            assert_eq!(value(6) - value(1), buy);
            assert_eq!(value(10) - value(5), sell);
            assert_eq!(value(11), value(1));
            assert_eq!((value(12), value(13)), (U256::ZERO, U256::ZERO));
        }
        assert_eq!(found, 4);
    }
}
