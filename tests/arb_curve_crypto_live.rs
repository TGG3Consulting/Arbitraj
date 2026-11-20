//! Файл: tests/arb_curve_crypto_live.rs
//! Что проверяет: решение 442 — crypto-пулы Curve Arbitrum One (Twocrypto-NG, Tricrypto-NG) на РЕАЛЬНЫХ ответах своего
//! узла Arbitrum, записанных на одном блоке скриптом tools/arbitrum_record/curve_crypto.py
//! (tests/data/arbitrum_curve_crypto_<дата>/twocrypto.json, tricrypto.json):
//! 1. Разбор ответов пула (pools/load_crypto.rs decode) — те же вызовы и тот же порядок, что у бота.
//! 2. Наша математика (math/crypto_swap.rs exchange_out: get_y Twocrypto / Tricrypto, комиссия с выхода) на записанном
//!    состоянии даёт ТОТ ЖЕ ответ, что get_dy пула (квотер сверки — quote/quoter_crypto.rs), на каждой сумме лесенки
//!    в обе стороны; откат get_dy — у нас None. Пул в рампе A/gamma (halted) не сверяется (бот его не котирует).
//! 3. Квотер сверки (quoter_crypto::decode) читает записанные ответы get_dy.
//!
//! Пока записи нет, тесты помечены #[ignore] — снять после записи со своего узла (данных узла Arbitrum у разработчика
//! нет; тесты — только на записанных ответах, CLAUDE.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::crypto_swap, pools::load_crypto, quote::quoter_crypto}. Моков нет.

use std::path::PathBuf;
use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::crypto_swap;
use arb::venues::bsc_dex::pools::load_crypto::{self, Raw};
use arb::venues::bsc_dex::quote::quoter_crypto;

/// Папка записи: последняя по имени tests/data/arbitrum_curve_crypto_<дата>. Паника — записи нет.
fn dir() -> PathBuf {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_curve_crypto_"))
        })
        .collect();
    found.sort();
    found.pop().expect("нет записи: tools/arbitrum_record/curve_crypto.py")
}

/// Ответ вызова из записи: {"result": hex} → байты, {"error": …} → откат с текстом.
fn piece(v: &Value) -> Result<Bytes, String> {
    match v.get("result").and_then(Value::as_str) {
        Some(h) => Bytes::from_str(h).map_err(|e| e.to_string()),
        None => Err(v.get("error").map(Value::to_string).unwrap_or_default()),
    }
}

/// Имена вызовов в порядке pools/load_crypto.rs calls(): HEAD записи, coins(k)/balances(k), price_scale.
fn order(doc: &Value, n: usize) -> Vec<String> {
    let mut out: Vec<String> = doc["head_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    for k in 0..n {
        out.push(format!("coins({k})"));
        out.push(format!("balances({k})"));
    }
    if n == 2 {
        out.push("price_scale()".to_string());
    } else {
        for k in 0..n - 1 {
            out.push(format!("price_scale({k})"));
        }
    }
    out
}

/// Сверка всех пулов одного файла: наш выход = get_dy пула. Принимает: имя файла, число монет.
fn check(name: &str, n: usize) {
    let path = dir().join(name);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let now = doc["timestamp"].as_u64().unwrap();
    let names = order(&doc, n);
    let (mut same, mut halted) = (0usize, 0usize);
    for p in doc["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        let got: Vec<Result<Bytes, String>> = names.iter().map(|k| piece(&p["calls"][k])).collect();
        let raw = Raw { pool, n, got, now };
        let (_, data) = load_crypto::decode(&raw).unwrap_or_else(|w| panic!("{pool}: {w}"));
        if data.halted {
            halted += 1;
            continue;
        }
        for r in p["get_dy"].as_array().unwrap() {
            let i = r["i"].as_u64().unwrap() as usize;
            let j = r["j"].as_u64().unwrap() as usize;
            let dx = U256::from_str(r["dx"].as_str().unwrap()).unwrap();
            let want = piece(&r["get_dy"]).ok().and_then(|b| quoter_crypto::decode(&b));
            let ours = crypto_swap::exchange_out(&data, i, j, dx);
            assert_eq!(ours, want, "{name} {pool}: i={i} j={j} dx={dx}");
            same += 1;
        }
    }
    assert!(same > 0, "{name}: нет ни одного ответа get_dy для сверки (пулов в рампе: {halted})");
}

/// Twocrypto-NG: наш выход = get_dy пула на каждой записанной сумме.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_crypto.py"]
fn twocrypto_matches_get_dy() {
    check("twocrypto.json", 2);
}

/// Tricrypto-NG: наш выход = get_dy пула на каждой записанной сумме.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_crypto.py"]
fn tricrypto_matches_get_dy() {
    check("tricrypto.json", 3);
}
