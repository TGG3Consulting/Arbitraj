//! Файл: tests/algebra_integral_common/mod.rs
//! Что делает: общее для тестов на записи Algebra Integral (tests/data/algebra_integral_2026-11-06/probe.json, блок
//! 132036268, два пула thena-v3): чтение записи, разбор слов ответа ABI и состояние пула с картой тиков — ровно как их
//! собирает бот (globalState: price, tick; fee(); liquidity(); ticks() по списку от текущего тика; окно карты —
//! load_algebra_integral::window по последним прочитанным тикам списка). Вынесено из tests/bsc_dex_algebra_integral_live.rs
//! (решение 323: тот же разбор нужен tests/bsc_dex_algebra_out_of_window.rs).
//! Кто вызывает: tests/bsc_dex_algebra_integral_live.rs, tests/bsc_dex_algebra_out_of_window.rs.
//! От чего зависит: arb::venues::bsc_dex::pools::{ticks, load_algebra_integral}, serde_json.
//! Настройки: нет.
// Каждый тест берёт часть общего — остальное для него не используется.
#![allow(dead_code)]

use std::collections::BTreeMap;

use alloy_primitives::U256;
use serde_json::Value;

use arb::venues::bsc_dex::pools::load_algebra_integral::window;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

/// Запись целиком. Ошибок нет (тест падает).
pub fn probe() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/algebra_integral_2026-11-06/probe.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Слово `i` ответа (беззнаковое).
pub fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Слово `i` как знаковое целое (int24 / int128 в слове ABI — дополнение до двух).
pub fn signed(raw: &str, i: usize) -> i128 {
    let w = word(raw, i);
    if w.bit(255) {
        let neg = (!w).wrapping_add(U256::from(1u8));
        -(neg.to::<u128>() as i128)
    } else {
        w.to::<u128>() as i128
    }
}

/// Состояние пула и карта тиков из записи: (цена, тик, ликвидность, комиссия, карта). Ошибок нет (тест падает).
pub fn pool_state(p: &Value) -> (U256, i32, u128, u32, TickMap) {
    let gs = p["calls"]["globalState"]["raw"].as_str().unwrap();
    let sqrt = word(gs, 0);
    let tick = signed(gs, 1) as i32;
    let fee_raw = p["calls"]["fee"]["raw"].as_str().unwrap();
    let fee = word(fee_raw, 0).to::<u32>();
    let liq = word(p["calls"]["liquidity"]["raw"].as_str().unwrap(), 0).to::<u128>();
    let mut map = BTreeMap::new();
    let mut ends = Vec::new();
    for side in ["ticks_up", "ticks_down"] {
        let list = p[side].as_array().unwrap();
        for t in list {
            let raw = t["raw"].as_str().unwrap();
            let total = word(raw, 0);
            if !total.is_zero() {
                map.insert(
                    t["tick"].as_i64().unwrap() as i32,
                    TickData {
                        gross: total.to::<u128>(),
                        net: signed(raw, 1),
                    },
                );
            }
        }
        ends.push(list.last().unwrap()["tick"].as_i64().unwrap() as i32);
    }
    let words = window(ends[1], ends[0]).expect("окно");
    (
        sqrt,
        tick,
        liq,
        fee,
        TickMap {
            spacing: 1,
            ticks: map.into(),
            words,
        },
    )
}
