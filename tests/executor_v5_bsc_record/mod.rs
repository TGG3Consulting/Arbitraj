//! Файл: tests/executor_v5_bsc_record/mod.rs
//! Что делает: сверка полученных сумм по операциям v5 с первичными ответами BSC-ноды. Тест вызывается из
//! bsc_dex_executor_v5_live.rs. Источники — raw_rpc.json и summary.json записи, события Swap менеджеров
//! Uniswap V4 и PancakeSwap Infinity CL. Для обоих видов token0/1 отсортированы по адресу.

use alloy_primitives::{Address, U256, keccak256};
use serde_json::Value;

use super::{BSC, Hook, Version, assert_record, class, deploy_gate, json, live_proof};

const DIR: &str = "tests/data/executor_v5_bsc_live_2026-11-23";
/// Блок 135328859 и пустой адрес X, на котором исполнили подменённый v5 (README записи).
const TAG: &str = "0x810f45b";
const X: &str = "0xd71e2e0aa8ad437e9ff2a95452c9a2749d69393b";

#[test]
fn bsc_runtime_and_receipts_bind_both_directions_to_live_pools() {
    assert_record(DIR, 56, TAG, X);
    let rows = json(DIR, "summary.json");
    let rows_array = rows.as_array().unwrap();
    assert_eq!(rows_array.len(), 8);
    for r in rows_array {
        assert_eq!(r["chain_id"], 56);
        assert_eq!(r["status"], 1);
        assert_eq!(r["x_left"]["usdt"], "0");
        assert_eq!(r["x_left"]["coin"], "0");
        match r["case"].as_str().unwrap() {
            "a" => {
                assert_eq!(r["got"], r["quoter_out"]);
                assert_eq!(r["sim_block_a"]["number"], "0x810f45c");
                assert_eq!(r["sim_block_quoter_a"], r["sim_block_a"]);
            }
            "b" => assert_eq!(r["got"], r["quoter_out_after_a"]),
            case => panic!("неожиданный вызов {case}"),
        }
    }
    check_swap_credits(&json(DIR, "raw_rpc.json"), &rows, X);
}

#[test]
fn bsc_deploy_accepts_only_the_two_proven_classes() {
    assert_eq!(live_proof::record_of(Version::V5, BSC), Some(DIR));
    let verdicts = live_proof::verdicts_of(Version::V5, BSC).as_ref().unwrap();
    for v in verdicts {
        let proven = [class(19, Hook::Without), class(20, Hook::Without)].contains(&v.class);
        assert_eq!(v.accepted, proven, "{}", v.why);
        let count = if proven { 2 } else { 0 };
        assert_eq!((v.proven, v.pools), (count, count));
    }
    assert_eq!(verdicts.iter().filter(|v| v.accepted).count(), 2);
    assert!(deploy_gate::refusal_at(BSC).is_none());
}

/// Сверяет результат шага на X с положительным кредитом выходного токена в событии менеджера этого пула.
fn check_swap_credits(raw: &Value, rows: &Value, executor: &str) {
    let mut swaps = Vec::new();
    for rpc in raw.as_array().unwrap() {
        if rpc["method"] != "eth_simulateV1" || rpc["result"].is_null() {
            continue;
        }
        let requests = rpc["params"][0]["blockStateCalls"][0]["calls"]
            .as_array()
            .unwrap();
        let replies = rpc["result"][0]["calls"].as_array().unwrap();
        assert_eq!(requests.len(), replies.len());
        for (req, reply) in requests.iter().zip(replies) {
            if req["to"].as_str().unwrap().eq_ignore_ascii_case(executor)
                && reply["status"] == "0x1"
            {
                swaps.push(reply);
            }
        }
    }
    for row in rows.as_array().unwrap() {
        if row["status"] != 1 || !matches!(row["case"].as_str(), Some("a" | "b")) {
            continue;
        }
        let sig = match row["kind"].as_u64().unwrap() {
            19 => "Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)",
            20 => "Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24,uint16)",
            kind => panic!("неожиданный вид шага в записи BSC: {kind}"),
        };
        let topic = keccak256(sig.as_bytes()).to_string();
        let token_in: Address = row["token_in"].as_str().unwrap().parse().unwrap();
        let token_out: Address = row["token_out"].as_str().unwrap().parse().unwrap();
        assert_ne!(token_in, token_out);
        let offset = if token_out < token_in { 2 } else { 66 };
        let amount = U256::from_str_radix(row["got"].as_str().unwrap(), 10).unwrap();
        assert!(amount > U256::ZERO && amount <= U256::from(i128::MAX as u128));
        let found = swaps.iter().any(|swap| {
            let returned = swap["returnData"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x");
            U256::from_str_radix(returned, 16).ok() == Some(amount)
                && swap["logs"].as_array().unwrap().iter().any(|log| {
                    if log["address"] != row["pool_manager"]
                        || log["topics"][0] != topic
                        || log["topics"][1] != row["pool_id"]
                    {
                        return false;
                    }
                    let credit = &log["data"].as_str().unwrap()[offset..offset + 64];
                    U256::from_str_radix(credit, 16).ok() == Some(amount)
                })
        });
        assert!(found, "нет результата исполнителя и кредита Swap для {row}");
    }
}
