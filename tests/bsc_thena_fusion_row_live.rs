//! Файл: tests/bsc_thena_fusion_row_live.rs
//! Что проверяет: ревью t235 (решение 461, T461-06) — вид thena_fusion (THENA Fusion, BSC) на записи своего узла BSC
//! (tools/bsc_record/thena_fusion.py → tests/data/bsc_thena_fusion_<дата>/; каждый пул — состояние и котировки на одном
//! блоке). Пулы THENA Fusion исполняются через ArbExecutor (шаг ALGEBRA), поэтому режим цели шага (NextTick::Row —
//! строка СЫРОГО битмапа, как AlgebraV1.9 TickTable.nextTickInTheSameRow) влияет на живые сделки BSC:
//! 1. pool_by_pair_and_code — poolByPair фабрики = пул; код всех пулов записи один (keccak).
//! 2. bitmap_is_raw_as_bot_reads — толкование битмапа, по которому читает бот (load_algebra.rs raw_ticks: бит b слова
//!    w = тик w·256 + b): у каждого установленного бита сырого окна liquidityTotal > 0. Факт Sourcify 56 (ревью t235):
//!    официальный AlgebraPool Deployer THENA 0xc89F…7358 собран из исходника с Constants.TICK_SPACING = 60 и
//!    TickTable со сжатием тика на TICK_SPACING (AlgebraV1) — если так и в коде пулов, этот тест упадёт (биты сырого
//!    окна — не тики), и нужен режим сжатого битмапа (вопрос Тиграну, TASKS T461-06).
//! 3. math_equals_pool_swap — расчёт бота (math/swap_algebra, режим Row, карта как load_algebra.rs) = ответу
//!    виртуального квотера (обмен самого пула в eth_call) и официального Quoter THENA (amountOut, fee), обе стороны, все
//!    суммы; при времени блока = времени последней точки оракула (обмен не пересчитывает комиссию).
//!
//! Под #[ignore] до записи (запись делает Тигран на своём узле BSC). Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::swap_algebra, pools::{abi_algebra, load_algebra, ticks},
//! quote::virtual_quoter}, serde_json.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState, NextTick};
use arb::venues::bsc_dex::pools::abi_algebra::IAlgebraPool;
use arb::venues::bsc_dex::pools::load_algebra::raw_ticks;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::virtual_quoter;

/// Папка последней записи (по дате в имени). Ошибок нет (тест падает: записи нет).
fn dir() -> PathBuf {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("bsc_thena_fusion_") && !n.ends_with(".zip"))
        })
        .collect();
    found.sort();
    found.pop().expect("нет записи: tools/bsc_record/thena_fusion.py")
}

/// Записи пулов (pool_<адрес>.json). Ошибок нет (тест падает).
fn pools() -> Vec<Value> {
    let d = dir();
    let mut v: Vec<PathBuf> = std::fs::read_dir(&d)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
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
fn result(j: &Value) -> Option<Bytes> {
    j.get("result")?.as_str()?.parse().ok()
}

/// Число из hex-строки записи. Ошибок нет (тест падает).
fn hex_u256(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

/// Сырые слова битмапа записи по порядку: (первое слово, последнее, слова). Ошибок нет (тест падает).
fn raw_words(p: &Value) -> (i16, i16, Vec<U256>) {
    let mut ws: Vec<(i16, U256)> = p["tickTable"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(w, j)| {
            let raw = result(j).expect("tickTable без ответа");
            let v = IAlgebraPool::tickTableCall::abi_decode_returns(&raw).unwrap();
            (w.parse::<i16>().unwrap(), v)
        })
        .collect();
    ws.sort_by_key(|(w, _)| *w);
    let (lo, hi) = (ws.first().unwrap().0, ws.last().unwrap().0);
    (lo, hi, ws.into_iter().map(|(_, v)| v).collect())
}

/// liquidityTotal и liquidityDelta тика из записи (поле ticks). Ошибок нет (тест падает: тик не записан).
fn tick_info(p: &Value, t: i32) -> IAlgebraPool::ticksReturn {
    let raw = result(&p["ticks"][t.to_string()]).expect("ticks() установленного бита записан");
    IAlgebraPool::ticksCall::abi_decode_returns(&raw).unwrap()
}

/// Карта сырых тиков, как load_algebra.rs tick_map (liquidityTotal > 0, spacing 1). Ошибок нет.
fn tick_map(p: &Value) -> TickMap {
    let (lo, hi, words) = raw_words(p);
    let mut map = BTreeMap::new();
    for t in raw_ticks(lo, &words) {
        let i = tick_info(p, t);
        if i.liquidityTotal > 0 {
            let d = TickData {
                gross: i.liquidityTotal,
                net: i.liquidityDelta,
            };
            map.insert(t, d);
        }
    }
    TickMap {
        spacing: 1,
        ticks: map.into(),
        words: (lo, hi),
    }
}

#[test]
#[ignore = "нет записи: tools/bsc_record/thena_fusion.py (T461-06)"]
fn pool_by_pair_and_code() {
    let mut codes = HashSet::new();
    for p in pools() {
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        let by_pair = result(&p["poolByPair"]).expect("poolByPair без ответа");
        assert_eq!(Address::from_slice(&by_pair[12..32]), pool, "poolByPair фабрики");
        codes.insert(p["code_keccak"].as_str().unwrap().to_string());
    }
    assert_eq!(codes.len(), 1, "код пулов записи разный: {codes:?}");
}

#[test]
#[ignore = "нет записи: tools/bsc_record/thena_fusion.py (T461-06)"]
fn bitmap_is_raw_as_bot_reads() {
    let mut bits = 0;
    for p in pools() {
        let (lo, _, words) = raw_words(&p);
        for t in raw_ticks(lo, &words) {
            let i = tick_info(&p, t);
            let pool = &p["pool"];
            assert!(
                i.liquidityTotal > 0,
                "{pool}: бит сырого тика {t} без ликвидности — битмап не сырой"
            );
            bits += 1;
        }
    }
    assert!(bits > 0, "в сырых окнах записи нет ни одного бита");
}

#[test]
#[ignore = "нет записи: tools/bsc_record/thena_fusion.py (T461-06)"]
fn math_equals_pool_swap() {
    let mut compared = 0;
    for p in pools() {
        let gs_raw = result(&p["globalState()"]).expect("globalState без ответа");
        let gs = IAlgebraPool::globalStateCall::abi_decode_returns(&gs_raw).unwrap();
        let liq_raw = result(&p["liquidity()"]).unwrap();
        let liquidity = IAlgebraPool::liquidityCall::abi_decode_returns(&liq_raw).unwrap();
        let map = tick_map(&p);
        let fee = u32::from(gs.fee);
        // Как load_algebra.rs load: PoolState::Algebra (fee — globalState.fee, fee_otz нет) и режим Row.
        let s = AlgebraState {
            sqrt_price: U256::from(gs.price),
            tick: gs.tick.as_i32(),
            liquidity,
            fee,
            ticks: &map,
            next: NextTick::Row,
        };
        for q in p["quotes"].as_array().unwrap() {
            let zfo = q["zero_for_one"].as_bool().unwrap();
            let amount = hex_u256(&q["amount_in"]);
            let ours = swap_algebra::swap(&s, zfo, true, amount).unwrap();
            // Расчёт вышел за прочитанное окно (--words записи) — честный неполный итог, сравнивать нечего.
            if !ours.complete {
                continue;
            }
            let at = &q["at_last_timepoint"];
            let pool = &p["pool"];
            if let Some(raw) = result(&at["virtual"]) {
                let out = virtual_quoter::decode(&raw, amount).unwrap();
                assert_eq!(out, Ok(ours.amount_out), "{pool} zfo={zfo} {amount}");
                compared += 1;
            }
            if let Some(raw) = result(&at["quoter"]).filter(|r| r.len() >= 64) {
                let out = U256::from_be_slice(&raw[..32]);
                let qfee = U256::from_be_slice(&raw[32..64]).to::<u32>();
                assert_eq!(qfee, fee, "{pool}: комиссия Quoter ≠ globalState.fee");
                assert_eq!(ours.amount_out, out, "{pool} Quoter zfo={zfo} {amount}");
            }
        }
    }
    assert!(compared > 0, "ни одного сравнения с обменом пула");
}
