//! Файл: tests/thena_fusion_common/mod.rs
//! Что делает: общее для тестов THENA Fusion на записях своего узла BSC (решение 463): чтение файлов pool_*.json записи
//! (tools/bsc_record/thena_fusion.py), слова ответов eth_call, карта тиков бота по СЖАТЫМ словам tickTable (как
//! pools/load_algebra.rs: row_ticks, тики с liquidityTotal > 0, окно — записанные строки), состояние для
//! math/swap_algebra (режим pool_quote::algebra_next(Proto::Algebra) = Row).
//! Кто вызывает: tests/bsc_thena_fusion_window_t236.rs (ревью t236: off_map и глубина на записи 20.11),
//! tests/bsc_thena_fusion_math_t237.rs и tests/bsc_thena_fusion_fee_t237.rs (запись t237),
//! tests/bsc_thena_fusion_fee_slot3.rs (запись со слотом 3).
//! От чего зависит: arb::venues::bsc_dex::{math::swap_algebra, pools::{families, load_algebra, ticks},
//! quote::pool_quote}; serde_json.
//! Настройки: нет.
// Каждый тест берёт часть общего — остальное для него не используется.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use alloy_primitives::{I256, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_algebra::AlgebraState;
use arb::venues::bsc_dex::pools::families::Proto;
use arb::venues::bsc_dex::pools::load_algebra::row_ticks;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::pool_quote::algebra_next;

/// Запись 20.11 (первая, 8 пулов; tests/bsc_thena_fusion_record.rs).
pub const FIRST: &str = "tests/data/bsc_thena_fusion_2026-11-20";
/// Запись t237 (60 пулов GeckoTerminal «thena-fusion», данные оракула комиссии).
pub const T237: &str = "tests/data/bsc_thena_fusion_2026-11-20_t237";
/// Повтор записи со слотом 3 (volumePerLiquidityInBlock), точками index + 2 и 0 и началом окна бота (T463-02).
pub const SLOT3: &str = "tests/data/bsc_thena_fusion_2026-11-20_slot3";

/// Файлы пулов записи `rel` (путь от корня репозитория), по имени. Принимает: путь, ожидаемое число пулов.
/// Ошибок нет (тест падает).
pub fn pools(rel: &str, n: usize) -> Vec<Value> {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    let mut names: Vec<PathBuf> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| is_pool(p))
        .collect();
    names.sort();
    assert_eq!(names.len(), n, "пулов в записи {rel}");
    names.iter().map(|p| read_json(p)).collect()
}

/// Файл пула записи (pool_<адрес>.json). Ошибок нет.
fn is_pool(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str());
    name.is_some_and(|n| n.starts_with("pool_"))
}

/// JSON файла. Ошибок нет (тест падает).
fn read_json(p: &Path) -> Value {
    let text = std::fs::read_to_string(p).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Слово `i` ответа eth_call (hex «0x…»). Ошибок нет (тест падает).
pub fn word(res: &Value, i: usize) -> U256 {
    let h = res.as_str().unwrap().trim_start_matches("0x");
    U256::from_str_radix(&h[i * 64..(i + 1) * 64], 16).unwrap()
}

/// Число слов ответа. Ошибок нет (тест падает).
pub fn len(res: &Value) -> usize {
    (res.as_str().unwrap().len() - 2) / 64
}

/// Слово как знаковое (int256 ABI). Ошибок нет.
pub fn signed(res: &Value, i: usize) -> I256 {
    I256::from_raw(word(res, i))
}

/// Число из hex-строки записи («0x…»). Ошибок нет (тест падает).
pub fn hex_u256(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

/// Слова битмапа записи ({позиция: ответ}) — по возрастанию позиции. Ошибок нет.
fn bitmap(p: &Value, key: &str) -> Vec<(i16, U256)> {
    let mut v: Vec<(i16, U256)> = p[key]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, r)| (k.parse().unwrap(), word(&r["result"], 0)))
        .collect();
    v.sort();
    v
}

/// Карта бота по сжатым словам записи — как load_algebra::load: row_ticks, тики с liquidityTotal > 0, окно — записанные
/// строки. Ошибок нет (тест падает).
pub fn map_of(p: &Value) -> TickMap {
    let spacing = p["tick_spacing"].as_i64().unwrap() as i32;
    let ws = bitmap(p, "tickTable_compressed");
    let first = ws[0].0;
    for (i, (w, _)) in ws.iter().enumerate() {
        assert_eq!(*w, first + i as i16, "слова записи подряд");
    }
    let vals: Vec<U256> = ws.iter().map(|(_, x)| *x).collect();
    let infos = &p["ticks_compressed"];
    let mut m = BTreeMap::new();
    for t in row_ticks(first, &vals, spacing) {
        let r = &infos[t.to_string()]["result"];
        let total = word(r, 0).to::<u128>();
        let delta = i128::try_from(signed(r, 1)).unwrap();
        assert!(total > 0, "{}: тик {t} без ликвидности", p["pool"]);
        let data = TickData {
            gross: total,
            net: delta,
        };
        m.insert(t, data);
    }
    TickMap {
        spacing,
        ticks: m.into(),
        words: (first, ws.last().unwrap().0),
    }
}

/// Состояние для math/swap_algebra из globalState / liquidity записи с комиссией `fee` (пипсы). Ошибок нет.
pub fn state<'a>(p: &Value, m: &'a TickMap, fee: u32) -> AlgebraState<'a> {
    let gs = &p["globalState()"]["result"];
    AlgebraState {
        sqrt_price: word(gs, 0),
        tick: signed(gs, 1).as_i32(),
        liquidity: word(&p["liquidity()"]["result"], 0).to::<u128>(),
        fee,
        ticks: m,
        next: algebra_next(Proto::Algebra),
    }
}

/// globalState.fee записи (пипсы). Ошибок нет.
pub fn gs_fee(p: &Value) -> u32 {
    word(&p["globalState()"]["result"], 2).to::<u32>()
}
