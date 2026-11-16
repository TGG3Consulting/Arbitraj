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
//! От чего зависит: tests/algebra_integral_common (запись, разбор, состояние пула), arb::venues::bsc_dex::math::swap_algebra.

mod algebra_integral_common;

use alloy_primitives::U256;

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState};

use algebra_integral_common::{pool_state, probe, word};

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
