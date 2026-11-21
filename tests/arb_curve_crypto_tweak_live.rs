//! Файл: tests/arb_curve_crypto_tweak_live.rs
//! Что проверяет: решение 472 — модель откатов tweak_price Twocrypto-NG / Tricrypto-NG (math/crypto_tweak2.rs, crypto_tweak3.rs) на записи своего узла
//! Arbitrum (tools/arbitrum_record/curve_crypto.py с вызовами состояния tweak_price — pools/load_crypto_tweak.rs):
//! 1. Ответы по calldata вызовов бота (load_crypto::calls + load_crypto_tweak::calls) разбираются decode бота, tweak
//!    прочитан (Some) у каждого пула Twocrypto-NG / Tricrypto-NG сверенной математики.
//! 2. На каждой сумме лесенки, где get_dy пула ответил, наш расчёт с моделью tweak_price даёт либо тот же выход, либо
//!    None ТОЛЬКО из-за tweak_price (без модели — тот же выход, что get_dy). Сколько сумм модель отклонила — в выводе
//!    теста (факт для разбора: get_dy пула tweak_price не проверяет — откат реального обмена этим тестом не доказан,
//!    TASKS 472).
//!
//! 3. Решение 472 (доделка, T472-02): симуляция exchange на тех же суммах (exchange_sim записи — eth_call с подменой
//!    баланса и разрешения, tools/arbitrum_record/curve_sim.py): пул отдал выход — наш расчёт с моделью тот же; пул
//!    откатил — наш расчёт None («модель говорит откат ↔ пул откатывает»).
//!
//! До записи с вызовами tweak — #[ignore] (в записи 2026-11-20 их нет).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/curve_crypto_common, arb::venues::bsc_dex::{pools::{load_crypto, load_crypto_tweak,
//! crypto_variant}, math::crypto_swap, quote::quoter_crypto}. Моков нет.

mod curve_crypto_common;

use std::str::FromStr;

use alloy_primitives::{Address, U256};

use arb::venues::bsc_dex::math::crypto_swap;
use arb::venues::bsc_dex::pools::{crypto_variant, load_crypto, load_crypto_tweak};
use arb::venues::bsc_dex::quote::quoter_crypto;
use curve_crypto_common::{answers, doc, piece};

/// Twocrypto-NG: tweak прочитан; с моделью — выход get_dy или отказ только из-за tweak_price.
#[test]
#[ignore = "нет записи с вызовами tweak: tools/arbitrum_record/curve_crypto.py"]
fn twocrypto_tweak_model_on_record() {
    check("twocrypto.json", 2);
}

/// Tricrypto-NG: то же.
#[test]
#[ignore = "нет записи с вызовами tweak: tools/arbitrum_record/curve_crypto.py"]
fn tricrypto_tweak_model_on_record() {
    check("tricrypto.json", 3);
}

/// Сверка записи name (n монет). Паника — расхождение или нет сверенных пулов.
fn check(name: &str, n: usize) {
    let d = doc(name);
    let now = d["timestamp"].as_u64().unwrap();
    let maths: Vec<Address> = d["maths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| Address::from_str(m.as_str().unwrap()).unwrap())
        .collect();
    // Как tests/arb_curve_crypto_live.rs: Twocrypto — maths[0] v2.1.0, maths[1] v2.0.0; Tricrypto — все NG.
    let (ng, v200) = if n == 2 {
        (vec![maths[0]], vec![maths[1]])
    } else {
        (maths.clone(), Vec::new())
    };
    let (mut same, mut refused, mut pools) = (0usize, 0usize, 0usize);
    let (mut sim_ok, mut sim_rev) = (0usize, 0usize);
    for p in d["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        let mut calls = load_crypto::calls(pool, n);
        calls.extend(load_crypto_tweak::calls(pool, n));
        let got = answers(p, &calls);
        let raw = load_crypto::Raw { pool, n, got, now };
        let (_, mut data, math) =
            load_crypto::decode(&raw).unwrap_or_else(|w| panic!("{pool}: {w}"));
        let Ok(v) = crypto_variant::variant(n, math, &v200, &ng) else {
            continue;
        };
        data.math = v;
        assert!(data.tweak.is_some(), "{pool}: tweak не прочитан");
        let mut bare = data.clone();
        bare.tweak = None;
        pools += 1;
        for r in p["get_dy"].as_array().unwrap() {
            let i = r["i"].as_u64().unwrap() as usize;
            let j = r["j"].as_u64().unwrap() as usize;
            let dx = U256::from_str(r["dx"].as_str().unwrap()).unwrap();
            let want = piece(&r["get_dy"])
                .ok()
                .and_then(|b| quoter_crypto::decode(&b));
            let Some(want) = want else {
                continue;
            };
            let plain = crypto_swap::exchange_out(&bare, i, j, dx);
            assert_eq!(plain, Some(want), "{pool} {i}->{j} {dx}");
            match crypto_swap::exchange_out(&data, i, j, dx) {
                Some(o) => {
                    assert_eq!(o, want, "{pool} {i}->{j} {dx}");
                    same += 1;
                }
                None => refused += 1,
            }
        }
        let (o, r) = sims(p, &data);
        (sim_ok, sim_rev) = (sim_ok + o, sim_rev + r);
    }
    println!("{name}: пулов {pools}, сумм с выходом {same}, отклонено tweak_price {refused}");
    println!("{name}: симуляций exchange — выход {sim_ok}, откат {sim_rev}");
    assert!(pools > 0 && same > 0, "нет сверенных пулов");
    assert!(sim_ok > 0, "нет симуляций exchange с выходом");
}

/// Симуляции exchange пула p: пул отдал выход — наш расчёт (с моделью tweak_price) тот же, откатил — None.
/// Строки без подтверждённой подмены пропускаются. Возвращает: (с выходом, с откатом).
fn sims(p: &serde_json::Value, data: &arb::core::types::CryptoData) -> (usize, usize) {
    let (mut ok, mut rev) = (0, 0);
    for s in p["exchange_sim"].as_array().into_iter().flatten() {
        if s["ok"].is_null() || s["override_ok"] == serde_json::Value::Bool(false) {
            continue;
        }
        let i = s["i"].as_u64().unwrap() as usize;
        let j = s["j"].as_u64().unwrap() as usize;
        let dx = U256::from_str(s["dx"].as_str().unwrap()).unwrap();
        let ours = crypto_swap::exchange_out(data, i, j, dx);
        let pool = &p["pool"];
        if s["ok"] == serde_json::Value::Bool(true) {
            let h = s["result"].as_str().unwrap().trim_start_matches("0x");
            let want = U256::from_str_radix(&h[..64.min(h.len())], 16).unwrap();
            assert_eq!(ours, Some(want), "{pool} {i}->{j} {dx}: пул отдал выход");
            ok += 1;
        } else {
            assert_eq!(
                ours, None,
                "{pool} {i}->{j} {dx}: пул откатил ({})",
                s["revert"]
            );
            rev += 1;
        }
    }
    (ok, rev)
}
