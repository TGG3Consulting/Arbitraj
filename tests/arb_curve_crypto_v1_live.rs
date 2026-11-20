//! Файл: tests/arb_curve_crypto_v1_live.rs
//! Что проверяет: решение 453 — crypto-пулы Curve первого поколения Arbitrum One (crypto v1 фабрики, tricrypto 2021)
//! на РЕАЛЬНЫХ ответах своего узла Arbitrum, записанных на одном блоке скриптом tools/arbitrum_record/curve_crypto.py
//! (tests/data/arbitrum_curve_crypto_<дата>/crypto_v1.json, tricrypto_v1.json); вынесено из
//! tests/arb_curve_crypto_live.rs (ревью 453: файлы не больше ~200 строк):
//! 1. Ответы — по calldata вызовов бота (pools/load_crypto_v1.rs calls()), разбор — decode бота (decimals из записи).
//! 2. Наш выход (math/crypto_swap.rs exchange_out) = get_dy пула на каждой сумме лесенки; остановленный пул
//!    (is_killed) не сверяется.
//! 3. У пулов фабрики crypto v1 код — прокси EIP-1167 на шаблон из записи (verify_crypto_v1.rs proxy_target).
//!
//! Запись tests/data/arbitrum_curve_crypto_2026-11-20 (решение 461): tricrypto_v1 — без #[ignore]; crypto_v1 — под
//! #[ignore] (у фабрики v1 на блоке записи pool_count = 0).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/curve_crypto_common, arb::venues::bsc_dex::pools::{load_crypto_v1, verify_crypto_v1}.
//! Моков нет.

mod curve_crypto_common;

use std::str::FromStr;

use alloy_primitives::Address;

use arb::venues::bsc_dex::pools::load_crypto_v1;
use arb::venues::bsc_dex::pools::verify_crypto_v1::proxy_target;
use curve_crypto_common::{answers, code, doc, ladder};

/// Пулы v1: разбор load_crypto_v1 (decimals из записи), наш выход = get_dy пула. Принимает: имя файла, число монет.
fn check_v1(name: &str, n: usize) {
    let d = doc(name);
    let now = d["timestamp"].as_u64().unwrap();
    let mut checked = 0;
    for p in d["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        let got = answers(p, &load_crypto_v1::calls(pool, n));
        let decimals: Vec<u8> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let raw = load_crypto_v1::Raw { pool, n, got, now, decimals };
        let (_, data) = load_crypto_v1::decode(&raw).unwrap_or_else(|w| panic!("{pool}: {w}"));
        if data.halted {
            continue;
        }
        if n == 2 {
            let t = proxy_target(&code(&p["code"])).expect("код пула v1 — не прокси EIP-1167");
            let templates = d["templates"].as_array().unwrap();
            assert!(templates.iter().any(|x| Address::from_str(x.as_str().unwrap()).unwrap() == t));
        }
        checked += ladder(name, pool, p, &data);
    }
    assert!(checked > 0, "{name}: нет ни одной сверенной суммы");
}

/// Фабрика crypto v1: наш выход = get_dy пула; код пула — прокси на шаблон из журнала развёртывания.
#[test]
#[ignore = "запись 2026-11-20: pool_count фабрики crypto v1 0xE5De15A9… = 0 — пулов нет, сверять нечего (TASKS T456-03)"]
fn crypto_v1_matches_get_dy() {
    check_v1("crypto_v1.json", 2);
}

/// Tricrypto 2021: наш выход = get_dy пула.
#[test]
fn tricrypto_v1_matches_get_dy() {
    check_v1("tricrypto_v1.json", 3);
}
