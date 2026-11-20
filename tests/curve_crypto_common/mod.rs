//! Файл: tests/curve_crypto_common/mod.rs
//! Что делает: общее для тестов crypto-пулов Curve Arbitrum на записанных ответах узла (решения 442, 453; разбито по
//! ревью 453 — файлы не больше ~200 строк): папка и файлы записи tools/arbitrum_record/curve_crypto.py, ответы по
//! calldata вызовов бота, код из записи и сверка хеша, сверка лесенки get_dy пула с нашей формулой.
//! Кто вызывает: tests/arb_curve_crypto_live.rs (NG), tests/arb_curve_crypto_v1_live.rs (v1).
//! От чего зависит: arb::venues::bsc_dex::{math::crypto_swap, quote::quoter_crypto}. Моков нет.
#![allow(dead_code)]

use std::path::PathBuf;
use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::Value;

use arb::core::types::CryptoData;
use arb::venues::bsc_dex::math::crypto_swap;
use arb::venues::bsc_dex::quote::quoter_crypto;

/// Папка записи: последняя по имени tests/data/arbitrum_curve_crypto_<дата>. Паника — записи нет.
pub fn dir() -> PathBuf {
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

/// Файл записи. Паника — нет файла.
pub fn doc(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

/// Ответ вызова из записи: {"result": hex} → байты, {"error": …} → откат с текстом.
pub fn piece(v: &Value) -> Result<Bytes, String> {
    match v.get("result").and_then(Value::as_str) {
        Some(h) => Bytes::from_str(h).map_err(|e| e.to_string()),
        None => Err(v.get("error").map(Value::to_string).unwrap_or_default()),
    }
}

/// Ответы на вызовы бота по их calldata. Паника — в записи нет вызова бота.
pub fn answers(p: &Value, calls: &[(Address, Bytes)]) -> Vec<Result<Bytes, String>> {
    calls
        .iter()
        .map(|(_, d)| {
            let key = format!("{d}");
            piece(p["calls"].get(&key).unwrap_or_else(|| panic!("в записи нет вызова бота {key}")))
        })
        .collect()
}

/// Код из записи {"code": hex}. Паника — нет кода.
pub fn code(v: &Value) -> Vec<u8> {
    Bytes::from_str(v["code"].as_str().expect("нет кода")).unwrap().to_vec()
}

/// Код (или его начало) совпадает с ожидаемым: длина и keccak256. Принимает: код, (длина, хеш без 0x), целиком ли.
pub fn same(code: &[u8], want: (usize, &str), whole: bool) -> bool {
    let ok_len = if whole { code.len() == want.0 } else { code.len() >= want.0 };
    ok_len && format!("{:x}", keccak256(&code[..want.0.min(code.len())])) == want.1
}

/// Сверка лесенки get_dy пула с нашей формулой. Возвращает: число сверенных сумм.
pub fn ladder(name: &str, pool: Address, p: &Value, data: &CryptoData) -> usize {
    let mut n = 0;
    for r in p["get_dy"].as_array().unwrap() {
        let i = r["i"].as_u64().unwrap() as usize;
        let j = r["j"].as_u64().unwrap() as usize;
        let dx = U256::from_str(r["dx"].as_str().unwrap()).unwrap();
        let want = piece(&r["get_dy"]).ok().and_then(|b| quoter_crypto::decode(&b));
        let ours = crypto_swap::exchange_out(data, i, j, dx);
        assert_eq!(ours, want, "{name} {pool}: i={i} j={j} dx={dx}");
        n += 1;
    }
    n
}
