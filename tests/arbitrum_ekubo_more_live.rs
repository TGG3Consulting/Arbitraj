//! Файл: tests/arbitrum_ekubo_more_live.rs
//! Что делает: решение 454 (Ekubo: нативный ETH, расширения, история PoolInitialized, хаб подписки) — проверки на
//! записи своего узла Arbitrum (tests/data/arbitrum_ekubo_<дата>/, пишет tools/arbitrum_record/ekubo_more.py; берётся
//! последняя по имени папка):
//! 1. topic0 PoolInitialized = значение, которым скрипт искал события (без записи — факт ABI);
//! 2. каждое PoolInitialized ядра: ключ события даёт PoolId события (как pools/history_ekubo.rs);
//! 3. пулы с нативным ETH: пара графа — обёртка (pools/ekubo_native.rs), наш расчёт в порядке ключа = Router.quote;
//! 4. пулы с TWAMM: виртуальные ордера к времени блока записи (math/ekubo_virtual.rs) + обмен = Router.quote (роутер
//!    исполняет их внутри сверки на том же блоке);
//! 5. замер нагрузки (core_load.json): логи ядра есть, PoolId обмена находится по данным (pools/ekubo_route.rs).
//!
//! Записи пока нет — тесты на запись помечены #[ignore]; после записи метку снимаем.
//! Кто вызывает: cargo test (п. 1), cargo test -- --ignored (п. 2–5).
//! От чего зависит: arb::venues::bsc_dex::{math, pools, quote}, файлы записи. Моков нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256, b256};
use serde_json::Value;

use arb::core::types::{EkuboData, EkuboTick, EkuboTwamm};
use arb::venues::bsc_dex::math::{ekubo_swap, ekubo_virtual};
use arb::venues::bsc_dex::pools::abi_ekubo::{self, IEkuboCore};
use arb::venues::bsc_dex::pools::{ekubo_native, ekubo_route, load_ekubo, load_ekubo_twamm};
use arb::venues::bsc_dex::quote::quoter_ekubo;

const SKIP: &str = "нет записи: tools/arbitrum_record/ekubo_more.py";

fn json(name: &str) -> Value {
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

fn hex(v: &Value) -> Vec<u8> {
    alloy_primitives::hex::decode(v.as_str().expect("hex")).expect("hex")
}

fn word(v: &Value) -> B256 {
    B256::from_slice(&hex(v))
}

fn addr(v: &Value) -> Address {
    v.as_str().expect("адрес").parse().expect("адрес")
}

/// Состояние пула из записи — как pools/load_ekubo.rs. native — обёртка вместо нативного ETH. Ошибок нет.
fn state(rec: &Value, block_ts: u64, native: Option<Address>) -> EkuboData {
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
fn u64s(v: &Value) -> Vec<u64> {
    let a = v.as_array().unwrap();
    a.iter().map(|x| x.as_u64().unwrap()).collect()
}

/// Состояние TWAMM из записи — как pools/load_ekubo_twamm.rs. Ошибок нет.
fn twamm(t: &Value) -> EkuboTwamm {
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
fn compare(rec: &Value, d: &EkuboData) -> usize {
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

#[test]
fn pool_initialized_topic() {
    let want = b256!("0x5e4688b340694b7c7fd30047fd082117dc46e32acfbf81a44bb1fac0ae65154d");
    assert_eq!(IEkuboCore::PoolInitialized::SIGNATURE_HASH, want);
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/ekubo_more.py"]
fn initialized_key_gives_pool_id() {
    let all = json("pool_initialized.json");
    let logs = all["logs"].as_array().unwrap();
    for raw in logs {
        let log: Log = serde_json::from_value(raw.clone()).expect("лог");
        let e = IEkuboCore::PoolInitialized::decode_log_data(&log.inner.data).expect("ABI");
        let k = e.poolKey;
        assert_eq!(abi_ekubo::pool_id(k.token0, k.token1, k.config), e.poolId);
        assert_eq!(ekubo_route::pool_id(&log), Some(e.poolId));
    }
    assert!(!logs.is_empty(), "PoolInitialized не найдено");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/ekubo_more.py"]
fn native_math_equals_router_quote() {
    let ts = json("more_meta.json")["timestamp"].as_u64().unwrap();
    // Обёртка в паре графа — любой ненулевой адрес ≠ token1 (сама математика идёт в порядке ключа).
    let w = Address::repeat_byte(0xee);
    let mut n = 0;
    for rec in json("native_pools.json").as_array().unwrap() {
        if rec.get("state").is_none() {
            continue;
        }
        let t1 = addr(&rec["token1"]);
        let pair = ekubo_native::pair(Address::ZERO, t1, Some(w));
        let (a, b, native) = pair.unwrap();
        assert_eq!(native, Some(w));
        let d = state(rec, ts, native);
        assert!(!ekubo_native::is_token1(&d, a, w), "вход обёрткой — token0 ключа");
        assert_eq!(ekubo_native::key_tokens(&d, a, b), (Address::ZERO, t1));
        n += compare(rec, &d);
    }
    assert!(n > 0, "ни одной сверки пулов с нативным ETH");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/ekubo_more.py"]
fn twamm_math_equals_router_quote() {
    let ts = json("more_meta.json")["timestamp"].as_u64().unwrap();
    let mut n = 0;
    for rec in json("twamm_pools.json").as_array().unwrap() {
        if rec.get("twamm").is_none() {
            continue;
        }
        let d = state(rec, ts, None);
        let at = ekubo_virtual::advance(&d, ts);
        let at = at.expect("виртуальные ордера к времени блока записи");
        n += compare(rec, &at);
    }
    assert!(n > 0, "ни одной сверки пулов с TWAMM");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/ekubo_more.py"]
fn core_load_logs_route() {
    let load = json("core_load.json");
    assert!(load["logs"].as_u64().unwrap() > 0, "логов ядра за окно замера нет");
    assert!(load["logs_per_block"].as_f64().is_some());
}
