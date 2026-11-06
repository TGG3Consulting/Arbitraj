//! Файл: tests/bsc_dex_algebra_integral_live.rs
//! Что проверяет: решение 188 — Algebra Integral (THENA V3) на РЕАЛЬНЫХ ответах узла BSC, записанных на одном блоке
//! 132036268 (tests/data/algebra_integral_2026-11-06/probe.json): два пула thena-v3 (BTCB/WBNB, SINGULARRY/WBNB).
//! 1. ABI: globalState (6 слов: price, tick, lastFee, pluginConfig, communityFee, unlocked) и ticks (6 слов: liquidityTotal,
//!    liquidityDelta, prevTick, nextTick, …) разбираются по integral-core 1.2.x; список тиков идёт от MIN_TICK до MAX_TICK.
//! 2. Окно карты (load_algebra_integral::window) накрывает текущий тик.
//! 3. Наша математика (math/swap_algebra.rs) на этой карте и комиссии fee() пула даёт тот же выход, что виртуальный
//!    квотер (настоящий swap пула с плагином, решение 186) — на 1, 100 и 10000 долларов, в обе стороны.
//!
//! 4. Решение 205: глубина в полосе (quote/depth_band_algebra.rs) на тех же пулах — обе стороны не нулевые, и обмен
//!    найденного входа без предела цены (без комиссии) даёт тот же выход (полоса — честный предел обмена).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::swap_algebra, pools::{ticks, load_algebra_integral}}, serde_json.

use std::collections::BTreeMap;

use alloy_primitives::U256;
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState};
use arb::venues::bsc_dex::pools::load_algebra_integral::window;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

fn probe() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/algebra_integral_2026-11-06/probe.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Слово `i` ответа (беззнаковое).
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Слово `i` как знаковое целое (int24 / int128 в слове ABI — дополнение до двух).
fn signed(raw: &str, i: usize) -> i128 {
    let w = word(raw, i);
    if w.bit(255) {
        let neg = (!w).wrapping_add(U256::from(1u8));
        -(neg.to::<u128>() as i128)
    } else {
        w.to::<u128>() as i128
    }
}

/// Состояние пула и карта тиков из записи.
fn pool_state(p: &Value) -> (U256, i32, u128, u32, TickMap) {
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

#[test]
fn abi_and_window_match_live_pools() {
    let d = probe();
    for p in d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["dex_id"] == "thena-v3")
    {
        let gs = p["calls"]["globalState"]["raw"].as_str().unwrap();
        // 6 слов globalState (integral-core 1.2.x), unlocked = 1.
        assert_eq!(gs.trim_start_matches("0x").len(), 64 * 6, "{}", p["pool"]);
        assert_eq!(word(gs, 5), U256::from(1u8));
        let (_, tick, _, _, m) = pool_state(p);
        let (lo, hi) = (i32::from(m.words.0) * 256, i32::from(m.words.1) * 256 + 255);
        assert!(
            lo <= tick && tick <= hi,
            "{}: тик {tick} вне [{lo}; {hi}]",
            p["pool"]
        );
    }
}

#[test]
fn local_math_equals_virtual_quoter() {
    let d = probe();
    for p in d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["dex_id"] == "thena-v3")
    {
        let (sqrt, tick, liq, fee, m) = pool_state(p);
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee,
            ticks: &m,
        };
        for q in p["quoter"].as_array().unwrap() {
            let raw = q["raw"].as_str().unwrap();
            let (used, out) = (word(raw, 0), word(raw, 1));
            let amount = U256::from_str_radix(&q["amountIn"].to_string(), 10).unwrap();
            assert_eq!(used, amount, "{}: квотер взял не весь вход", p["pool"]);
            let zfo = q["zeroForOne"].as_bool().unwrap();
            let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
            assert!(
                r.complete,
                "{} {zfo} {}: не исполнено целиком",
                p["pool"], q["usd"]
            );
            assert_eq!(
                r.amount_out, out,
                "{} zeroForOne={zfo} ${}: наша математика ≠ виртуальный квотер",
                p["pool"], q["usd"]
            );
        }
    }
}

#[test]
fn band_depth_algebra_on_live_pools() {
    let d = probe();
    for p in d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["dex_id"] == "thena-v3")
    {
        let (sqrt, tick, liq, _, m) = pool_state(p);
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee: 0,
            ticks: &m,
        };
        for zfo in [true, false] {
            let (a_in, a_out, _) =
                arb::venues::bsc_dex::quote::depth_band_algebra::algebra(&s, zfo, 0.02, !zfo)
                    .expect("глубина Algebra");
            assert!(
                !a_in.is_zero() && !a_out.is_zero(),
                "{} {zfo}: глубина 0",
                p["pool"]
            );
            let r = swap_algebra::swap(&s, zfo, true, a_in).expect("обмен");
            let diff = if r.amount_out > a_out {
                r.amount_out - a_out
            } else {
                a_out - r.amount_out
            };
            assert!(
                diff <= a_out / U256::from(1_000_000u32) + U256::from(1u8),
                "{} {zfo}: обмен {} ≠ глубина {}",
                p["pool"],
                r.amount_out,
                a_out
            );
        }
    }
}
