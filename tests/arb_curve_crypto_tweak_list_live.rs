//! Файл: tests/arb_curve_crypto_tweak_list_live.rs
//! Что проверяет: решение 472 (T472-03) — модель откатов tweak_price пулов Curve первого поколения без фабрики:
//! tricrypto 2021 (math/crypto_tweak_v1_3.rs) и пара EURS/2CRV (math/crypto_tweak_v1.rs, factory = false) — чтение
//! pools/load_crypto_tweak_list.rs, на записи своего узла Arbitrum (tools/arbitrum_record/curve_crypto.py, поле
//! tweak_v1 у tricrypto_v1.json и crypto2_v1.json):
//! 1. Номер слота not_adjusted (37 — Vyper 0.2.15, 36 — Vyper 0.3.0): соседние слоты = геттеры (slot_check) —
//!    после этого load_crypto_tweak_list::CONFIRMED ставится в true (решение — по этому тесту).
//! 2. Ответы на вызовы бота разбираются decode бота, tweak прочитан.
//! 3. Лесенка get_dy: без модели — выход get_dy; с моделью — тот же выход или None только из-за tweak_price.
//! 4. Симуляция exchange: пул отдал выход — наш расчёт с моделью тот же; пул откатил — None.
//!
//! До записи с tweak_v1 у этих пулов — #[ignore].
//! Кто вызывает: cargo test.
//! От чего зависит: tests/curve_crypto_common, arb::venues::bsc_dex::{pools::{load_crypto_v1,
//! load_crypto_tweak_list}, math::crypto_swap, quote::quoter_crypto}. Моков нет.

mod curve_crypto_common;

use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::crypto_swap;
use arb::venues::bsc_dex::pools::load_crypto_tweak_list as tl;
use arb::venues::bsc_dex::pools::load_crypto_v1;
use arb::venues::bsc_dex::quote::quoter_crypto;
use curve_crypto_common::{answers, doc, piece};

/// Tricrypto 2021: слот 37 и модель.
#[test]
#[ignore = "нет записи с tweak_v1 у tricrypto_v1.json: tools/arbitrum_record/curve_crypto.py"]
fn tricrypto_v1_tweak_model_on_record() {
    check("tricrypto_v1.json", 3);
}

/// EURS/2CRV: слот 36 и модель.
#[test]
#[ignore = "нет записи crypto2_v1.json с tweak_v1: tools/arbitrum_record/curve_crypto.py --crypto2-v1-pool"]
fn crypto2_v1_tweak_model_on_record() {
    check("crypto2_v1.json", 2);
}

/// Сверка записи name (n монет). Паника — расхождение или нет сверенных сумм.
fn check(name: &str, n: usize) {
    let d = doc(name);
    let now = d["timestamp"].as_u64().unwrap();
    let (mut same, mut refused, mut sims) = (0, 0, 0);
    for p in d["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        let t = &p["tweak_v1"];
        for (k, ok) in t["slot_check"].as_object().unwrap() {
            assert_eq!(*ok, Value::Bool(true), "{pool}: слот {k} ≠ геттер");
        }
        let mut got = answers(p, &load_crypto_v1::calls_with(pool, n, true));
        let base = got.len();
        let token = Address::from_str(t["token"].as_str().unwrap()).unwrap();
        let (coins, _) = load_crypto_v1::decode(&raw(pool, n, got.clone(), now, p)).unwrap();
        for (to, data) in tl::calls(pool, n, token, &coins) {
            got.push(source(p, pool, token, to, &data));
        }
        got.push(Ok(word(&t["storage"][tl::slot(n).to_string()])));
        let r = raw(pool, n, got, now, p);
        let (_, mut data) = load_crypto_v1::decode(&r).unwrap_or_else(|w| panic!("{pool}: {w}"));
        data.tweak = tl::decode(&r.got[base..], n, now).unwrap();
        assert!(data.tweak.is_some(), "{pool}: tweak не прочитан");
        if data.halted {
            continue;
        }
        let mut bare = data.clone();
        bare.tweak = None;
        for row in p["get_dy"].as_array().unwrap() {
            let i = row["i"].as_u64().unwrap() as usize;
            let j = row["j"].as_u64().unwrap() as usize;
            let dx = U256::from_str(row["dx"].as_str().unwrap()).unwrap();
            let want = piece(&row["get_dy"])
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
        for s in p["exchange_sim"].as_array().into_iter().flatten() {
            if s["ok"].is_null() || s["override_ok"] == Value::Bool(false) {
                continue;
            }
            let i = s["i"].as_u64().unwrap() as usize;
            let j = s["j"].as_u64().unwrap() as usize;
            let dx = U256::from_str(s["dx"].as_str().unwrap()).unwrap();
            let ours = crypto_swap::exchange_out(&data, i, j, dx);
            if s["ok"] == Value::Bool(true) {
                let h = s["result"].as_str().unwrap().trim_start_matches("0x");
                let want = U256::from_str_radix(&h[..64.min(h.len())], 16).unwrap();
                assert_eq!(ours, Some(want), "{pool} {i}->{j} {dx}: пул отдал выход");
            } else {
                assert_eq!(
                    ours, None,
                    "{pool} {i}->{j} {dx}: пул откатил ({})",
                    s["revert"]
                );
            }
            sims += 1;
        }
    }
    println!("{name}: сумм с выходом {same}, отклонено tweak_price {refused}, симуляций {sims}");
    assert!(same > 0, "{name}: нет сверенных сумм");
}

/// Raw бота из ответов (decimals — из записи). Ошибок нет.
fn raw(
    pool: Address,
    n: usize,
    got: Vec<Result<Bytes, String>>,
    now: u64,
    p: &Value,
) -> load_crypto_v1::Raw {
    let decimals: Vec<u8> = p["decimals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    load_crypto_v1::Raw {
        pool,
        n,
        got,
        now,
        decimals,
        killed: true,
    }
}

/// Ответ вызова tweak из записи по адресу: пул — calls, токен — token_calls, монета — holdings. Паника — нет ответа.
fn source(
    p: &Value,
    pool: Address,
    token: Address,
    to: Address,
    data: &Bytes,
) -> Result<Bytes, String> {
    let key = format!("{data}");
    let v = if to == pool {
        &p["calls"][&key]
    } else if to == token {
        &p["tweak_v1"]["token_calls"][&key]
    } else {
        let h = p["holdings"].as_object().unwrap();
        h.iter()
            .find(|(c, _)| Address::from_str(c).ok() == Some(to))
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("нет balanceOf монеты {to}"))
    };
    assert!(!v.is_null(), "в записи нет ответа {to} {key}");
    piece(v)
}

/// Слово слота 0x… → 32 байта. Паника — не число.
fn word(v: &Value) -> Bytes {
    let h = v.as_str().unwrap().trim_start_matches("0x");
    let n = U256::from_str_radix(if h.is_empty() { "0" } else { h }, 16).unwrap();
    Bytes::from(n.to_be_bytes::<32>().to_vec())
}
