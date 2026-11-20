//! Файл: tests/ekubo_more_common/mod.rs
//! Что делает: общее для tests/arbitrum_ekubo_more_live.rs (решение 454) — чтение файлов записи
//! tools/arbitrum_record/ekubo_more.py (последняя по имени папка tests/data/arbitrum_ekubo_<дата>/), состояние пула и
//! TWAMM из записи (как pools/load_ekubo.rs, pools/load_ekubo_twamm.rs) и сверка нашего обмена с Router.quote.
//! Кто вызывает: tests/arbitrum_ekubo_more_live.rs.
//! От чего зависит: arb::core::types, arb::venues::bsc_dex::{math, pools, quote}, файлы записи. Моков нет.
#![allow(dead_code)]

use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

use arb::core::types::{EkuboData, EkuboTick, EkuboTwamm};
use arb::venues::bsc_dex::math::ekubo_swap;
use arb::venues::bsc_dex::pools::{abi_ekubo, load_ekubo, load_ekubo_twamm};
use arb::venues::bsc_dex::quote::quoter_ekubo;

pub const SKIP: &str = "нет записи: tools/arbitrum_record/ekubo_more.py";

pub fn json(name: &str) -> Value {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut found: Vec<_> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(name).exists())
        .filter(|p| p.to_string_lossy().contains("arbitrum_ekubo_"))
        .collect();
    found.sort();
    let text = std::fs::read_to_string(found.pop().expect(SKIP).join(name)).expect(name);
    serde_json::from_str(&text).expect(name)
}

pub fn hex(v: &Value) -> Vec<u8> {
    alloy_primitives::hex::decode(v.as_str().expect("hex")).expect("hex")
}

pub fn word(v: &Value) -> B256 {
    B256::from_slice(&hex(v))
}

pub fn addr(v: &Value) -> Address {
    v.as_str().expect("адрес").parse().expect("адрес")
}

/// Состояние пула из записи — как pools/load_ekubo.rs. native — обёртка вместо нативного ETH. Ошибок нет.
pub fn state(rec: &Value, block_ts: u64, native: Option<Address>) -> EkuboData {
    let (sqrt_ratio, tick, liquidity) = abi_ekubo::decode_state(&word(&rec["state"]));
    let w = |i: usize| rec["words"].get(i).and_then(Value::as_u64).unwrap_or(0);
    let bitmap: Vec<(u64, U256)> = rec["bitmap"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| (e["word"].as_u64().unwrap(), U256::from_be_bytes(word(&e["bits"]).0)))
                .collect()
        })
        .unwrap_or_default();
    let ticks: Vec<EkuboTick> = rec["ticks"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    let (net, delta) = abi_ekubo::decode_tick(&word(&e["info"]));
                    let tick = e["tick"].as_i64().unwrap() as i32;
                    EkuboTick { tick, net, delta }
                })
                .collect()
        })
        .unwrap_or_default();
    let mev = rec.get("mev_state").map(|v| load_ekubo::mev_state(&word(v)));
    EkuboData {
        config: word(&rec["config"]),
        sqrt_ratio,
        tick,
        liquidity,
        words: (w(0), w(1)),
        bitmap,
        ticks,
        mev,
        block_ts,
        native,
        twamm: rec.get("twamm").map(twamm),
    }
}

/// Список u64 из массива записи. Ошибок нет (паника — не тот вид).
pub fn u64s(v: &Value) -> Vec<u64> {
    let a = v.as_array().unwrap();
    a.iter().map(|x| x.as_u64().unwrap()).collect()
}

/// Состояние TWAMM из записи — как pools/load_ekubo_twamm.rs. Ошибок нет.
pub fn twamm(t: &Value) -> EkuboTwamm {
    let (last, rate0, rate1) = load_ekubo_twamm::decode_state(&word(&t["state"]));
    let words = u64s(&t["words"]);
    let raw = t["bitmap"].as_array().unwrap();
    let mut bitmap = Vec::new();
    for (w, b) in words.iter().zip(raw) {
        let b = word(b);
        if !b.is_zero() {
            bitmap.push((*w, U256::from_be_bytes(b.0)));
        }
    }
    let mut times = Vec::new();
    for x in t["times"].as_array().unwrap() {
        let (d0, d1) = load_ekubo_twamm::decode_time(&word(&x["info"]));
        times.push((x["time"].as_u64().unwrap(), d0, d1));
    }
    EkuboTwamm {
        last,
        rate0,
        rate1,
        words,
        bitmap,
        times,
        until: t["until"].as_u64().unwrap(),
    }
}

/// Наш расчёт = Router.quote по всем полным расчётам записи пула. Возвращает: число сверок. Ошибок нет (паника).
pub fn compare(rec: &Value, d: &EkuboData) -> usize {
    let mut n = 0;
    for q in rec["quotes"].as_array().into_iter().flatten() {
        let amount: u128 = q["amount"].as_str().unwrap().parse().unwrap();
        let is_token1 = q["is_token1"].as_bool().unwrap();
        let ours = ekubo_swap::exact_in(d, is_token1, amount);
        if let (Some(out), Some(r)) = (q.get("out"), ours) {
            if !r.complete || r.amount_in != amount || d.mev.is_some() {
                continue;
            }
            let router = quoter_ekubo::decode(&hex(out)).map(|v| v.to::<u128>());
            assert_eq!(router, Some(r.amount_out), "пул {} вход {amount}", rec["pool_id"]);
            n += 1;
        }
    }
    n
}
