//! Файл: tests/bsc_dex_algebra_out_of_window.rs
//! Что проверяет: решение 323 — текущий тик пула Algebra вне прочитанного окна карты тиков (после Swap, до
//! перечитывания) даёт честный неполный итог с нулями (complete = false; глубина — 0, не дошли), как V3
//! (math/swap_cl.rs), а не шаг к краю окна по другую сторону цены и не постоянную ликвидность на непрочитанном отрезке.
//! Запись — Algebra Integral, блок 132036268 (tests/data/algebra_integral_2026-11-06/probe.json, два пула thena-v3).
//! Карта сужена до одного слова сырого битмапа вокруг записанного тика (ровно то, что дало бы чтение меньшего отрезка
//! списка тиков): окно [lo; hi] = [w·256; w·256 + 255]. Тик «после Swap» — за краем окна или на краю, цена — цена этого
//! тика по TickMath (как tests/bsc_dex_off_map_algebra.rs moved): это состояние пула, не ответ площадки.
//! 1. Записанный тик в узком окне — выход равен ЗАПИСАННОМУ ответу виртуального квотера ($1 и $100, обе стороны):
//!    правка внутри окна ничего не меняет.
//! 2. Тик hi + 1 и lo − 1 — обмен в обе стороны и глубина в полосе 2% в обе стороны — нули, complete/дошли = false.
//! 3. Тик ровно hi, цена внутри тика hi: вверх — нули (следующий тик hi + 1 за окном, как V3 next_tick), вниз — считается.
//! 4. Пустое окно (первое слово после последнего) — нули при записанном тике.
//!
//! THENA Fusion (Algebra V1/V1.9) — та же функция math/swap_algebra.rs swap_to (PoolState::Algebra, quote/pool_quote.rs и
//! quote/depth_band_algebra.rs), своей записи нет.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/algebra_integral_common, arb::venues::bsc_dex::{math::{swap_algebra, tick_math},
//! pools::ticks, quote::depth_band_algebra}.

mod algebra_integral_common;

use alloy_primitives::U256;
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState, Swapped};
use arb::venues::bsc_dex::math::tick_math::sqrt_ratio_at_tick;
use arb::venues::bsc_dex::pools::ticks::TickMap;
use arb::venues::bsc_dex::quote::depth_band_algebra;

use algebra_integral_common::{pool_state, probe, word};

/// Честный неполный итог (решение 323).
const NONE: Swapped = Swapped {
    amount_in: U256::ZERO,
    amount_out: U256::ZERO,
    complete: false,
};

/// Карта, суженная до слов `w` (включительно): тики вне [w.0·256; w.1·256 + 255] отброшены. Ошибок нет.
fn narrowed(m: &TickMap, w: (i16, i16)) -> TickMap {
    let (lo, hi) = (i32::from(w.0) * 256, i32::from(w.1) * 256 + 255);
    TickMap {
        spacing: m.spacing,
        ticks: m
            .ticks
            .iter()
            .filter(|(t, _)| (lo..=hi).contains(*t))
            .map(|(t, d)| (*t, *d))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into(),
        words: w,
    }
}

/// Пул thena-v3 записи: запись пула, цена, тик, ликвидность, комиссия, карта в одном слове вокруг тика.
struct Pool<'a> {
    p: &'a Value,
    sqrt: U256,
    tick: i32,
    liq: u128,
    fee: u32,
    m: TickMap,
}

/// Пулы thena-v3 записи. Ошибок нет (тест падает).
fn pools(d: &Value) -> Vec<Pool<'_>> {
    d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["dex_id"] == "thena-v3")
        .map(|p| {
            let (sqrt, tick, liq, fee, m) = pool_state(p);
            let w = (tick >> 8) as i16;
            Pool {
                p,
                sqrt,
                tick,
                liq,
                fee,
                m: narrowed(&m, (w, w)),
            }
        })
        .collect()
}

/// Ответы квотера записи на $1 и $100: (zeroForOne, вход, выход).
fn small_quotes(p: &Value) -> Vec<(bool, U256, U256)> {
    p["quoter"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["usd"] == 1 || q["usd"] == 100)
        .map(|q| {
            let raw = q["raw"].as_str().unwrap();
            let amount = U256::from_str_radix(&q["amountIn"].to_string(), 10).unwrap();
            (q["zeroForOne"].as_bool().unwrap(), amount, word(raw, 1))
        })
        .collect()
}

#[test]
fn in_window_unchanged_against_recorded_quoter() {
    let d = probe();
    for Pool {
        p,
        sqrt,
        tick,
        liq,
        fee,
        m,
    } in pools(&d)
    {
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee,
            ticks: &m,
        };
        for (zfo, amount, out) in small_quotes(p) {
            let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
            assert!(r.complete, "{} {zfo}: в узком окне не исполнено", p["pool"]);
            assert_eq!(
                r.amount_out, out,
                "{} {zfo}: ≠ записанный квотер",
                p["pool"]
            );
        }
    }
}

#[test]
fn tick_outside_window_gives_zero_incomplete() {
    let d = probe();
    for Pool {
        p,
        tick,
        liq,
        fee,
        m,
        ..
    } in pools(&d)
    {
        let w = (tick >> 8) as i16;
        let (lo, hi) = (i32::from(w) * 256, i32::from(w) * 256 + 255);
        for t in [hi + 1, lo - 1] {
            let sqrt = sqrt_ratio_at_tick(t).unwrap();
            let s = AlgebraState {
                sqrt_price: sqrt,
                tick: t,
                liquidity: liq,
                fee,
                ticks: &m,
            };
            for (zfo, amount, _) in small_quotes(p) {
                let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
                assert_eq!(r, NONE, "{} тик {t} {zfo}: не нули", p["pool"]);
            }
            let s0 = AlgebraState { fee: 0, ..s };
            for zfo in [true, false] {
                let got = depth_band_algebra::algebra(&s0, zfo, 0.02, !zfo).expect("глубина");
                assert_eq!(
                    got,
                    (U256::ZERO, U256::ZERO, false),
                    "{} тик {t} {zfo}",
                    p["pool"]
                );
            }
        }
    }
}

#[test]
fn tick_on_upper_edge_up_is_incomplete_down_counts() {
    let d = probe();
    for Pool {
        p,
        tick,
        liq,
        fee,
        m,
        ..
    } in pools(&d)
    {
        let hi = i32::from((tick >> 8) as i16) * 256 + 255;
        // Цена внутри тика hi (ниже цены тика hi + 1): globalState.tick = hi.
        let sqrt = sqrt_ratio_at_tick(hi + 1).unwrap() - U256::from(1u8);
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick: hi,
            liquidity: liq,
            fee,
            ticks: &m,
        };
        for (zfo, amount, _) in small_quotes(p) {
            let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
            if zfo {
                assert!(
                    !r.amount_in.is_zero(),
                    "{}: вниз от hi не считается",
                    p["pool"]
                );
            } else {
                assert_eq!(r, NONE, "{}: вверх от hi не нули", p["pool"]);
            }
        }
        let s0 = AlgebraState { fee: 0, ..s };
        let up = depth_band_algebra::algebra(&s0, false, 0.02, true).expect("глубина");
        assert_eq!(up, (U256::ZERO, U256::ZERO, false), "{}", p["pool"]);
    }
}

#[test]
fn empty_window_gives_zero_incomplete() {
    let d = probe();
    for Pool {
        p,
        sqrt,
        tick,
        liq,
        fee,
        m,
    } in pools(&d)
    {
        let w = (tick >> 8) as i16;
        let empty = narrowed(&m, (w + 1, w));
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee,
            ticks: &empty,
        };
        for (zfo, amount, _) in small_quotes(p) {
            let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
            assert_eq!(r, NONE, "{} {zfo}: пустое окно не нули", p["pool"]);
        }
    }
}
