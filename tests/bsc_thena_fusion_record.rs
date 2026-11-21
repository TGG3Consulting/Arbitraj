//! Файл: tests/bsc_thena_fusion_record.rs
//! Что проверяет: решение 463 — математика бота для THENA Fusion (вид thena_fusion, код AlgebraV1: сжатый битмап
//! tickTable на TICK_SPACING) на записи своего узла BSC 20.11 (tests/data/bsc_thena_fusion_2026-11-20, блок 134740226,
//! 8 пулов GeckoTerminal «thena-fusion»; запись tools/bsc_record/thena_fusion.py ветки feat/arbitrum dbdbe2be).
//! 1. Факт битмапа: у 7 из 8 пулов записанные СЫРЫЕ слова (tick >> 8) пусты, у всех 8 СЖАТЫЕ слова дают тики
//!    (load_algebra::row_ticks) и у каждого из них ticks().liquidityTotal > 0 — карта бота строится по сжатым словам.
//! 2. Обмен бота (quote/pool_quote::algebra_next(Proto::Algebra) = NextTick::Row, math/swap_algebra::swap на карте из
//!    сжатых слов, комиссия globalState.fee) равен до wei ответам Quoter THENA (0xeA68…b2cA, quoteExactInputSingle)
//!    и виртуального квотера записи, снятым при времени последнего timepoint пула (`at_last_timepoint` — комиссия не
//!    пересчитывается, как у бота) — на всех суммах (1, 10, 100, 1000 б.п. резерва) и обеих сторонах, где обмен
//!    уложился в записанные строки (63 из 64 случаев). Один случай (0xd405…668b, 1 → 0, 1000 б.п.) выходит за 5
//!    записанных строк — у бота честный неполный итог (complete = false) с выходом не больше квотера.
//! 3. Динамическая комиссия: у пула 0x1b93…2210 Quoter при времени блока записи (`at_block`, первый своп в новом
//!    блоке — _getNewFee) вернул комиссию 1539, а globalState.fee = 1537 — факт для задачи T463-02 (комиссия
//!    следующего блока), тест его фиксирует.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::swap_algebra, pools::{load_algebra, ticks, families},
//! quote::pool_quote}; serde_json.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use alloy::sol_types::SolCall;
use alloy_primitives::{I256, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState, NextTick};
use arb::venues::bsc_dex::pools::abi_algebra::IAlgebraPool;
use arb::venues::bsc_dex::pools::families::Proto;
use arb::venues::bsc_dex::pools::load_algebra::row_ticks;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::pool_quote::algebra_next;

/// Папка записи. Ошибок нет.
fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/bsc_thena_fusion_2026-11-20")
}

/// Файлы пулов записи (8 штук, по имени). Ошибок нет (тест падает).
fn pools() -> Vec<Value> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| is_pool(p))
        .collect();
    names.sort();
    assert_eq!(names.len(), 8, "пулов в записи");
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
fn word(res: &Value, i: usize) -> U256 {
    let h = res.as_str().unwrap().trim_start_matches("0x");
    U256::from_str_radix(&h[i * 64..(i + 1) * 64], 16).unwrap()
}

/// Слово как знаковое (int256 ABI). Ошибок нет.
fn signed(res: &Value, i: usize) -> I256 {
    I256::from_raw(word(res, i))
}

/// Слова битмапа записи ({позиция: ответ}) — по возрастанию позиции. Ошибок нет.
fn words(p: &Value, key: &str) -> Vec<(i16, U256)> {
    let mut v: Vec<(i16, U256)> = p[key]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, r)| (k.parse().unwrap(), word(&r["result"], 0)))
        .collect();
    v.sort();
    v
}

/// ticks(int24) записи: тик → (liquidityTotal, liquidityDelta). Ошибок нет.
fn tick_infos(p: &Value, key: &str) -> BTreeMap<i32, (u128, i128)> {
    p[key]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, r)| {
            let total = word(&r["result"], 0).to::<u128>();
            let delta = i128::try_from(signed(&r["result"], 1)).unwrap();
            (k.parse().unwrap(), (total, delta))
        })
        .collect()
}

/// Карта бота по сжатым словам записи — как load_algebra::load: row_ticks, тики с liquidityTotal > 0. Ошибок нет.
fn map_of(p: &Value) -> TickMap {
    let spacing = p["tick_spacing"].as_i64().unwrap() as i32;
    let ws = words(p, "tickTable_compressed");
    let first = ws[0].0;
    for (i, (w, _)) in ws.iter().enumerate() {
        assert_eq!(*w, first + i as i16, "слова записи подряд");
    }
    let vals: Vec<U256> = ws.iter().map(|(_, x)| *x).collect();
    let infos = tick_infos(p, "ticks_compressed");
    let mut m = BTreeMap::new();
    for t in row_ticks(first, &vals, spacing) {
        let (total, delta) = infos[&t];
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

#[test]
fn bitmap_is_compressed() {
    let mut raw_empty = 0;
    for p in pools() {
        let name = p["pool"].as_str().unwrap();
        assert_eq!(p["tick_spacing"].as_i64(), Some(60), "{name}");
        let raw = words(&p, "tickTable");
        if raw.iter().all(|(_, x)| x.is_zero()) {
            raw_empty += 1;
        }
        let m = map_of(&p);
        assert!(!m.ticks.is_empty(), "{name}: сжатая карта пуста");
        let aligned = m.ticks.keys().all(|t| t % 60 == 0);
        assert!(aligned, "{name}: тик не кратен шагу");
    }
    assert_eq!(raw_empty, 7, "сырые слова пусты у 7 из 8 пулов");
}

#[test]
fn bot_math_equals_thena_quoter() {
    assert_eq!(algebra_next(Proto::Algebra), NextTick::Row);
    let (mut exact, mut partial) = (0, 0);
    for p in pools() {
        let gs = &p["globalState()"]["result"];
        let m = map_of(&p);
        let s = AlgebraState {
            sqrt_price: word(gs, 0),
            tick: signed(gs, 1).as_i32(),
            liquidity: word(&p["liquidity()"]["result"], 0).to::<u128>(),
            fee: word(gs, 2).to::<u32>(),
            ticks: &m,
            next: algebra_next(Proto::Algebra),
        };
        for q in p["quotes"].as_array().unwrap() {
            let zfo = q["zero_for_one"].as_bool().unwrap();
            let hex = q["amount_in"].as_str().unwrap().trim_start_matches("0x");
            let amount = U256::from_str_radix(hex, 16).unwrap();
            let (qr, vr) = (
                &q["at_last_timepoint"]["quoter"],
                &q["at_last_timepoint"]["virtual"],
            );
            let quoter = word(&qr["result"], 0);
            let name = p["pool"].as_str().unwrap();
            assert_eq!(word(&qr["result"], 1).to::<u32>(), s.fee, "{name}");
            let (v_in, v_out) = (word(&vr["result"], 0), word(&vr["result"], 1));
            assert_eq!(v_out, quoter, "{name}: виртуальный квотер = Quoter");
            let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
            let case = format!("{} zfo={zfo} bps={}", p["pool"], q["bps"]);
            if r.complete {
                assert_eq!((r.amount_in, r.amount_out), (v_in, v_out), "{case}");
                exact += 1;
            } else {
                // За записанными строками — честный неполный итог, не больше квотера.
                assert!(r.amount_out < quoter && r.amount_in < amount, "{case}");
                partial += 1;
            }
        }
    }
    assert_eq!((exact, partial), (63, 1));
}

#[test]
fn first_swap_in_block_changes_fee() {
    let p = pools()
        .into_iter()
        .find(|p| p["pool"] == "0x1b932598da273069e1878e4a1505b6f833462210")
        .unwrap();
    let fee = word(&p["globalState()"]["result"], 2).to::<u32>();
    let q = &p["quotes"][0];
    assert_eq!(fee, 1537);
    let at_last = word(&q["at_last_timepoint"]["quoter"]["result"], 1);
    let at_block = word(&q["at_block"]["quoter"]["result"], 1);
    assert_eq!(at_last.to::<u32>(), 1537);
    assert_eq!(at_block.to::<u32>(), 1539);
}

/// Ответы globalState() записи разбираются ABI бота (abi_algebra.rs: communityFeeToken0/1 — uint16, решение 463) в те же
/// цену, тик и комиссию, что и по словам. Ошибок нет (тест падает).
#[test]
fn global_state_abi_on_record() {
    for p in pools() {
        let res = &p["globalState()"]["result"];
        let raw = hex::decode(res.as_str().unwrap().trim_start_matches("0x")).unwrap();
        let gs = IAlgebraPool::globalStateCall::abi_decode_returns(&raw).unwrap();
        assert_eq!(U256::from(gs.price), word(res, 0), "{}", p["pool"]);
        assert_eq!(gs.tick.as_i32(), signed(res, 1).as_i32(), "{}", p["pool"]);
        assert_eq!(u32::from(gs.fee), word(res, 2).to::<u32>(), "{}", p["pool"]);
        assert!(gs.unlocked, "{}", p["pool"]);
    }
}
