//! Файл: tests/algebra_dir_common/mod.rs
//! Что делает: решение 444 — общее для тестов на записи своего узла Arbitrum вида algebra_dir_fee (Algebra V1.9 с
//! направленными комиссиями, Camelot V3): поиск папки записи tests/data/arbitrum_algebra_dir_fee_<дата>/ (пишет
//! tools/arbitrum_record/algebra_dir_fee.py), чтение файлов, разбор ответов узла и состояние пула с картой сырых
//! тиков — теми же функциями, что у бота (load_algebra_dir::state_of, load_algebra::row_ticks при шаге 1 — сырой
//! битмап).
//! Кто вызывает: tests/arb_dex_algebra_dir.rs, tests/arb_dex_algebra_dir_events.rs.
//! От чего зависит: arb::venues::bsc_dex::pools::{abi_algebra_dir, abi_algebra, load_algebra, load_algebra_dir, ticks},
//! serde_json.
//! Настройки: нет.
// Каждый тест берёт часть общего — остальное для него не используется.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use alloy::sol_types::SolCall;
use alloy_primitives::Bytes;
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::pools::abi_algebra::IAlgebraPool;
use arb::venues::bsc_dex::pools::abi_algebra_dir::IAlgebraDirPool;
use arb::venues::bsc_dex::pools::load_algebra::row_ticks;
use arb::venues::bsc_dex::pools::load_algebra_dir::state_of;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

/// Метка ignore тестов до записи (снимается после записи со своего узла).
pub const NO_RECORD: &str = "нет записи: tools/arbitrum_record/algebra_dir_fee.py";

/// Папка последней записи (по дате в имени). Ошибок нет (тест падает: записи нет).
pub fn dir() -> PathBuf {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_algebra_dir_fee_"))
        })
        .collect();
    found.sort();
    found.pop().expect(NO_RECORD)
}

/// Файл записи как JSON. Ошибок нет (тест падает).
pub fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

/// Записи пулов (pool_<адрес>.json). Ошибок нет.
pub fn pools() -> Vec<Value> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir())
        .unwrap()
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("pool_"))
        })
        .collect();
    v.sort();
    v.iter()
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
        .collect()
}

/// Байты ответа eth_call (поле result); None — ответ с ошибкой (откат). Ошибок нет.
pub fn result(j: &Value) -> Option<Bytes> {
    j.get("result")?.as_str()?.parse().ok()
}

/// globalState пула записи по ABI направленных комиссий (8 значений). Ошибок нет (тест падает).
pub fn global_state(p: &Value) -> IAlgebraDirPool::globalStateReturn {
    let raw = result(&p["globalState()"]).expect("globalState без ответа");
    assert_eq!(raw.len(), 8 * 32, "globalState: 8 слов по ABI ветки direction-based-fee");
    IAlgebraDirPool::globalStateCall::abi_decode_returns(&raw).unwrap()
}

/// Состояние пула (как load_algebra_dir.rs) и карта сырых тиков (как load_algebra.rs tick_map). Ошибок нет.
pub fn pool_state(p: &Value) -> (PoolState, TickMap) {
    let gs = global_state(p);
    let liq_raw = result(&p["liquidity()"]).unwrap();
    let liquidity = IAlgebraPool::liquidityCall::abi_decode_returns(&liq_raw).unwrap();
    let state = state_of(&gs, liquidity);
    let mut ws: Vec<(i16, alloy_primitives::U256)> = p["tickTable"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(w, j)| {
            let raw = result(j).unwrap();
            let v = IAlgebraPool::tickTableCall::abi_decode_returns(&raw).unwrap();
            (w.parse::<i16>().unwrap(), v)
        })
        .collect();
    ws.sort_by_key(|(w, _)| *w);
    let lo = ws.first().unwrap().0;
    let hi = ws.last().unwrap().0;
    let words: Vec<_> = ws.iter().map(|(_, v)| *v).collect();
    let mut map = BTreeMap::new();
    for t in row_ticks(lo, &words, 1) {
        let raw = result(&p["ticks"][t.to_string()]).expect("ticks() установленного бита записан");
        let i = IAlgebraPool::ticksCall::abi_decode_returns(&raw).unwrap();
        if i.liquidityTotal > 0 {
            let d = TickData {
                gross: i.liquidityTotal,
                net: i.liquidityDelta,
            };
            map.insert(t, d);
        }
    }
    let m = TickMap {
        spacing: 1,
        ticks: map.into(),
        words: (lo, hi),
    };
    (state, m)
}
