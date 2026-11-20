//! Файл: tests/arbitrum_saddle_live.rs
//! Что делает: решение 451 (T448-07) — пулы Saddle на Arbitrum (saddle-contract 2cfdd055) на записи своего узла:
//! (1) swapStorage() — 7 слов, swapFee берётся abi_saddle::swap_fee; (2) разбор и сборка состояния тем же кодом, что у
//! бота (load_saddle::parse / build — шаблон A_PRECISION = 100, 256 раундов, откат); (3) наш выход
//! (math/stableswap_v1::exchange_out_t) = calculateSwap пула на каждой записанной сумме и паре (i, j).
//! Кто вызывает: cargo test (запись tests/data/arbitrum_saddle_2026-11-20, решение 461).
//! От чего зависит: tests/data/arbitrum_saddle_<дата>/ (tools/arbitrum_record/saddle.py),
//! arb::venues::bsc_dex::{pools::{abi_saddle, load_saddle}, math::stableswap_v1}.

use std::path::PathBuf;

use alloy_primitives::{Address, Bytes, U256, hex};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t};
use arb::venues::bsc_dex::pools::abi_saddle::swap_fee;
use arb::venues::bsc_dex::pools::load_saddle;

/// Последняя по имени папка записи tests/data/arbitrum_saddle_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_saddle_"))
        })
        .collect();
    dirs.sort();
    dirs.pop().expect("нет записи: tools/arbitrum_record/saddle.py")
}

/// Ответ {"ok": "0x…"} → байты; откат — Err(текст).
fn answer(v: &Value) -> Result<Bytes, String> {
    match v.get("ok").and_then(|h| h.as_str()) {
        Some(h) => Ok(Bytes::from(hex::decode(h).expect("hex"))),
        None => Err(v["error"].as_str().unwrap_or("нет ответа").to_string()),
    }
}

fn bad(what: String) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, None, what)
}

/// Пулы записи без ошибки чтения.
fn pools() -> Vec<Value> {
    let text = std::fs::read_to_string(data_dir().join("pools.json")).expect("pools.json");
    let all: Vec<Value> = serde_json::from_str(&text).expect("json");
    all.into_iter().filter(|p| p.get("error").is_none()).collect()
}

/// swapStorage() у пулов Saddle Arbitrum — 7 слов (struct Swap без полей Nerve), swapFee разбирается.
#[test]
fn swap_storage_is_seven_words() {
    for p in pools() {
        let raw = answer(&p["swapStorage"]).expect("swapStorage");
        assert_eq!(raw.len(), 7 * 32, "пул {}", p["pool"]);
        assert!(swap_fee(&raw).is_some());
    }
}

/// Состояние тем же разбором, что у бота, и наш выход = calculateSwap пула.
#[test]
fn math_matches_calculate_swap() {
    let mut checked = 0;
    for p in pools() {
        let tokens = p["getToken"].as_array().unwrap();
        let mut head: Vec<Result<Bytes, String>> = Vec::new();
        for (k, t) in tokens.iter().enumerate() {
            head.push(answer(t));
            head.push(answer(&p["getTokenBalance"][k]));
        }
        head.push(Err("Out of range".into()));
        let tail = vec![
            answer(&p["getAPrecise"]),
            answer(&p["swapStorage"]),
            answer(&p["paused"]),
        ];
        let coin = |k: usize| Address::from_slice(&answer(&tokens[k]).unwrap()[12..]);
        let (a, b) = (coin(0), coin(1));
        let pair = if a < b { (a, b) } else { (b, a) };
        let parsed = load_saddle::parse(&head, &tail, pair, &bad).unwrap();
        let dec: Vec<u8> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| U256::from_be_slice(&answer(d).unwrap()).to::<u8>())
            .collect();
        let PoolState::StableSwap(st) = load_saddle::build(parsed, &dec, &bad).unwrap() else {
            panic!("не StableSwap");
        };
        let t = Template {
            amp_precision: st.amp_precision,
            max_rounds: st.max_rounds,
            strict: st.strict,
        };
        for q in p["quotes"].as_array().unwrap() {
            let Ok(raw) = answer(q) else { continue };
            let want = U256::from_be_slice(&raw);
            let (i, j) = (q["i"].as_u64().unwrap() as usize, q["j"].as_u64().unwrap() as usize);
            let dx: U256 = q["dx"].as_str().unwrap().parse().unwrap();
            let ours = exchange_out_t(i, j, dx, &st.balances, &st.rates, st.amp, st.fee, &t);
            assert_eq!(ours, Some(want), "пул {} {i}→{j} dx {dx}", p["pool"]);
            checked += 1;
        }
    }
    assert!(checked > 0, "ни одного ответа calculateSwap в записи");
}
