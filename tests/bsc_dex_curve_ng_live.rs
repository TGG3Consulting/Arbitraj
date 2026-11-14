//! Файл: tests/bsc_dex_curve_ng_live.rs
//! Что проверяет: решение 275 — Curve StableSwap-NG (curve-bsc) на РЕАЛЬНЫХ ответах узла BSC, записанных на одном
//! блоке 133594967 (tests/data/curve_ng_2026-11-14/probe.json): три plain-пула фабрики NG 0xd7E7…988B —
//! rtUSQ/USDT 0xc2d5…d563 (виды монет [2 rebasing, 0]), WBNB/USDT 0x4723…04cb ([1 оракул курса, 0]; курс WBNB в
//! stored_rates ≈ 589,9·1e18) и USDT/USDC/FDUSD 0xa5e0…8ec4 ([0, 0, 0]).
//! 1. Версия пула — "v7.0.0" (version() — та, по исходнику которой написан порт).
//! 2. Реестр фабрики: get_coins(pool) — монеты пула, is_meta = false → verify_curve_ng::decide принимает пару; чужая
//!    пара (монеты не из пула) — отказ.
//! 3. Наша математика NG (math/stableswap_ng.rs::exchange_received) на балансах, stored_rates, A_precise, fee и
//!    offpeg_fee_multiplier этого блока даёт ТОТ ЖЕ выход, что get_dy пула — 1, 100 и 10000 единиц, все пары индексов.
//! 4. Реальный лог TokenExchange пула — topic0 NG (5 полей, как Curve 2020) и он в topics() вида.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::stableswap_ng, pools::{abi_curve_ng, families, verify, verify_curve_ng}}.

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::stableswap_ng;
use arb::venues::bsc_dex::pools::abi_curve_ng;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::verify_curve_ng::decide;

fn probe() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/curve_ng_2026-11-14/probe.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn hex(s: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).unwrap()
}

/// Слово `i` ответа.
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

fn addr(raw: &str) -> Address {
    Address::from_slice(&hex(raw)[12..32])
}

#[test]
fn version_is_v7() {
    for p in probe()["pools"].as_array().unwrap() {
        let v = hex(p["calls"]["version"].as_str().unwrap());
        // Строка ABI: смещение, длина, байты.
        let len = word(p["calls"]["version"].as_str().unwrap(), 1).to::<usize>();
        assert_eq!(&v[64..64 + len], b"v7.0.0", "{}", p["pool"]);
    }
}

#[test]
fn factory_registry_accepts_pool_pair() {
    let fam = families::make("curve-bsc", "curve_stableswap_ng").expect("вид");
    for p in probe()["pools"].as_array().unwrap() {
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        let coins: Vec<Address> = p["coins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| addr(c.as_str().unwrap()))
            .collect();
        let f = &p["factory"];
        let answers: Vec<Result<Bytes, String>> = ["get_coins", "is_meta"]
            .iter()
            .map(|k| Ok(Bytes::from(hex(f[*k].as_str().unwrap()))))
            .collect();
        let cand = |coin: Address, quote: Address| Candidate {
            family: fam,
            pool: p["pool"].as_str().unwrap().to_string(),
            coin,
            quote,
        };
        let found = decide(&cand(coins[0], coins[1]), pool, &answers).expect("пул принят");
        assert_eq!(found.coin, coins[0]);
        // Монета не из пула (адрес самого пула) — отказ.
        assert!(decide(&cand(pool, coins[1]), pool, &answers).is_err());
    }
}

#[test]
fn ng_math_equals_pool_get_dy() {
    for p in probe()["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let n = word(c["N_COINS"].as_str().unwrap(), 0).to::<usize>();
        let amp = word(c["A_precise"].as_str().unwrap(), 0);
        let fee = word(c["fee"].as_str().unwrap(), 0);
        let offpeg = word(c["offpeg"].as_str().unwrap(), 0);
        let sr = c["stored_rates"].as_str().unwrap();
        assert_eq!(word(sr, 1).to::<usize>(), n);
        let rates: Vec<U256> = (0..n).map(|k| word(sr, 2 + k)).collect();
        let balances: Vec<U256> = p["balances"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| word(b.as_str().unwrap(), 0))
            .collect();
        for g in p["get_dy"].as_array().unwrap() {
            let (i, j) = (
                g["i"].as_u64().unwrap() as usize,
                g["j"].as_u64().unwrap() as usize,
            );
            let dx = U256::from_str_radix(g["dx"].as_str().unwrap(), 10).unwrap();
            let want = word(g["raw"].as_str().unwrap(), 0);
            let (got, _) =
                stableswap_ng::exchange_received(i, j, dx, &balances, &rates, amp, fee, offpeg)
                    .expect("расчёт NG");
            assert_eq!(
                got, want,
                "{} {i}→{j} dx={dx}: математика ≠ get_dy",
                p["pool"]
            );
        }
    }
}

#[test]
fn token_exchange_log_is_known_topic() {
    let d = probe();
    let logs = d["logs"].as_array().unwrap();
    assert!(!logs.is_empty());
    let known = abi_curve_ng::topics();
    for l in logs {
        let t0 = l["topics"][0].as_str().unwrap().parse().unwrap();
        assert!(known.contains(&t0), "topic0 {t0} не из событий NG");
    }
}
