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
//! Запись 21.11.2026 (tests/data/arbitrum_ekubo_2026-11-21, блок 526041266, решение 479): п. 2, 3, 5 — без #[ignore]
//! (PoolInitialized 22, пулы с нативным ETH — 9 полных расчётов = Router.quote, логов ядра за 100 001 блок 12); п. 4 — под
//! #[ignore] с причиной-фактом записи.
//! Кто вызывает: cargo test (п. 1–3, 5), cargo test -- --ignored (п. 4).
//! От чего зависит: tests/ekubo_more_common (чтение записи, состояние пула, сверка с Router.quote),
//! arb::venues::bsc_dex::{math, pools}, файлы записи. Моков нет.

mod ekubo_more_common;

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, b256};

use arb::venues::bsc_dex::math::ekubo_virtual;
use arb::venues::bsc_dex::pools::abi_ekubo::{self, IEkuboCore};
use arb::venues::bsc_dex::pools::{ekubo_native, ekubo_route};
use ekubo_more_common::{addr, compare, json, state};

#[test]
fn pool_initialized_topic() {
    let want = b256!("0x5e4688b340694b7c7fd30047fd082117dc46e32acfbf81a44bb1fac0ae65154d");
    assert_eq!(IEkuboCore::PoolInitialized::SIGNATURE_HASH, want);
}

#[test]
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
        assert!(
            !ekubo_native::is_token1(&d, a, w),
            "вход обёрткой — token0 ключа"
        );
        assert_eq!(ekubo_native::key_tokens(&d, a, b), (Address::ZERO, t1));
        n += compare(rec, &d);
    }
    assert!(n > 0, "ни одной сверки пулов с нативным ETH");
}

#[test]
#[ignore = "записи 2026-11-20 и 2026-11-21 (526041266): у всех 3 пулов TWAMM ликвидность 0, скорости ордеров 0, времён ордеров нет — полных расчётов нет, сверять нечего (TASKS T477-03)"]
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
fn core_load_logs_route() {
    let load = json("core_load.json");
    assert!(
        load["logs"].as_u64().unwrap() > 0,
        "логов ядра за окно замера нет"
    );
    assert!(load["logs_per_block"].as_f64().is_some());
}
