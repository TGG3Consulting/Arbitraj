//! Файл: tests/arb_curve_crypto_tweak_v1_live.rs
//! Что проверяет: решение 472 (T472-03) — модель откатов tweak_price пула crypto v1 фабрики Curve (CurveCryptoSwap2ETH;
//! math/crypto_tweak_v1.rs, pools/load_crypto_tweak_v1.rs) на записи своего узла Arbitrum
//! (tools/arbitrum_record/curve_crypto.py, crypto_v1.json, поле tweak_v1):
//! 1. Слот not_adjusted = 33 по раскладке Vyper 0.3.1: соседние слоты 28, 31, 32, 34 = геттеры D(), xcp_profit_a(),
//!    virtual_price(), admin_actions_deadline() (slot_check записи) — номер слота подтверждён фактом узла.
//! 2. Ответы по вызовам бота (load_crypto_v1::calls + load_crypto_tweak_v1::calls, слово слота 33, ETH пула)
//!    разбираются decode бота, tweak прочитан.
//! 3. Лесенка get_dy: без модели — тот же выход, что get_dy; с моделью — тот же выход или None только из-за
//!    tweak_price (сколько — в выводе).
//! 4. Симуляция exchange (exchange_sim): пул отдал выход — наш расчёт с моделью тот же; пул откатил — None.
//!
//! Монета WETH20 пула — по get_eth_index фабрики (Factory.vy L422-L431), как load_crypto_tweak_v1.rs statics. До записи с tweak_v1 — #[ignore] (на
//! 20.11.2026 pool_count фабрики crypto v1 Arbitrum = 0 — TASKS T456-03).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/curve_crypto_common, arb::venues::bsc_dex::{pools::{load_crypto_v1, load_crypto_tweak_v1},
//! math::crypto_swap, quote::quoter_crypto}. Моков нет.

mod curve_crypto_common;

use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::crypto_swap;
use arb::venues::bsc_dex::pools::load_crypto_tweak_v1::{self as tv1, Statics};
use arb::venues::bsc_dex::pools::load_crypto_v1;
use arb::venues::bsc_dex::quote::quoter_crypto;
use curve_crypto_common::{answers, doc, piece};

/// Пулы фабрики crypto v1: слот, разбор, лесенка и симуляции (см. шапку).
#[test]
#[ignore = "нет записи с tweak_v1: tools/arbitrum_record/curve_crypto.py (пулов фабрики v1 на 20.11 нет)"]
fn crypto_v1_tweak_model_on_record() {
    let d = doc("crypto_v1.json");
    let now = d["timestamp"].as_u64().unwrap();
    let (mut pools, mut same, mut refused, mut sim_ok, mut sim_rev) = (0, 0, 0, 0, 0);
    for p in d["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        let t = &p["tweak_v1"];
        for s in ["28", "31", "32", "34"] {
            assert_eq!(
                t["slot_check"][s],
                Value::Bool(true),
                "{pool}: слот {s} ≠ геттер"
            );
        }
        let st = tv1::pre_decode(&answers(p, &tv1::pre_calls(pool))).unwrap();
        let mut got = answers(p, &load_crypto_v1::calls(pool, 2));
        // Ответы прежних вызовов — 12 (HEAD 7, монеты 4, price_scale 1); дальше — tweak (load_crypto_v1.rs state).
        assert_eq!(got.len(), 12);
        for (to, data) in tv1::calls(pool, &st) {
            got.push(source(p, &st, pool, to, &data));
        }
        got.push(Ok(word(&t["storage"]["33"])));
        got.push(Ok(word(&p["holdings"]["eth_balance"])));
        let decimals: Vec<u8> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let raw = load_crypto_v1::Raw {
            pool,
            n: 2,
            got,
            now,
            decimals,
            killed: false,
        };
        let (coins, mut data) =
            load_crypto_v1::decode(&raw).unwrap_or_else(|w| panic!("{pool}: {w}"));
        let k = piece(&p["factory_get_eth_index"]).unwrap();
        let k = U256::from_be_slice(&k[..32]);
        // Нет монеты WETH (MAX_UINT256) — gulp — balanceOf обеих (load_crypto_tweak_v1.rs statics).
        let eth = (k < U256::from(2u8)).then(|| k.to::<usize>());
        data.tweak = tv1::decode(&raw.got[12..], &coins, eth).unwrap();
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
                sim_ok += 1;
            } else {
                assert_eq!(
                    ours, None,
                    "{pool} {i}->{j} {dx}: пул откатил ({})",
                    s["revert"]
                );
                sim_rev += 1;
            }
        }
    }
    println!("crypto v1: пулов {pools}, сумм с выходом {same}, отклонено tweak_price {refused}");
    println!("crypto v1: симуляций exchange — выход {sim_ok}, откат {sim_rev}");
    assert!(pools > 0 && same > 0, "нет сверенных пулов");
}

/// Ответ вызова tweak из записи по адресу: пул — calls, токен — token_calls, фабрика — factory_calls, монета —
/// holdings[монета] (balanceOf пула). Паника — в записи нет ответа.
fn source(
    p: &Value,
    st: &Statics,
    pool: Address,
    to: Address,
    data: &Bytes,
) -> Result<Bytes, String> {
    let key = format!("{data}");
    let t = &p["tweak_v1"];
    let v = if to == pool {
        &p["calls"][&key]
    } else if to == st.token {
        &t["token_calls"][&key]
    } else if to == st.factory {
        &t["factory_calls"][&key]
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

/// Число-строка 0x… (слово слота или баланс) → 32 байта. Паника — не число.
fn word(v: &Value) -> Bytes {
    let h = v.as_str().unwrap().trim_start_matches("0x");
    let n = U256::from_str_radix(if h.is_empty() { "0" } else { h }, 16).unwrap();
    Bytes::from(n.to_be_bytes::<32>().to_vec())
}
