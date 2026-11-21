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
//! 2. Тик hi + 1 — обмен и глубина в полосе 2% в обе стороны — нули, complete/дошли = false; тик lo − 1 — то же вниз.
//! 3. Тик ровно hi, цена внутри тика hi: вверх — нули (следующий тик hi + 1 за окном, как V3 next_tick), вниз — считается.
//! 4. Тик lo − 1, цена тика lo − 1, ликвидность этого отрезка (записанная минус liquidityDelta тиков [lo; записанный] —
//!    как после Swap вниз): вверх считается (lo − 1 ≤ tick < hi, как V3 next_tick: позиция tick + 1 = lo в окне) и
//!    равен тому же обмену на ПОЛНОЙ карте записи (независимый путь: окно полной карты шире, край узкого не участвует) —
//!    $1 и $100 до цены тика hi и глубина 2% вверх.
//!    В узком окне (слово записанного тика) ни у одного из двух пулов записи инициализированных тиков нет (пул 0xd1d3b6…,
//!    тик 46884: окно [46848; 47103], ближайшие тики 46560 и 47220; пул 0x26399a…, тик −115601: окно
//!    [−115712; −115457], ближайшие −887220 и −114660), поэтому этот случай проверяет только, что проверка окна
//!    пропускает tick = lo − 1 и итог равен расчёту на полной карте; вычитание liquidityDelta (сумма пуста) и
//!    пересечение тиков на этих данных не задействованы — это делает случай 6.
//! 5. Пустое окно (первое слово после последнего) — нули при записанном тике.
//! 6. Окно — слово ближайшего записанного тика НИЖЕ записанного (пул 0xd1d3b6…: тик 46560, слово 181, окно
//!    [46336; 46591], в нём записанные тики 46500 и 46560): тик lo − 1 = 46335, ликвидность — записанная минус
//!    liquidityDelta тиков 46500 и 46560 (= 702598871493693393262 — ровно liquidityDelta записанного тика 45960, нижней
//!    границы этого отрезка); вверх $1 и $100 до цены тика hi и глубина 2% вверх (≈198 тиков, до 46533 — пересекает
//!    записанный тик 46500 внутри окна) равны расчёту на полной карте. У пула 0x26399a… ближайший тик ниже — −887220,
//!    слово начинается ниже MIN_TICK (lo − 1 вне TickMath) — случай для него не строится.
//!
//! THENA Fusion (Algebra V1/V1.9) — та же функция math/swap_algebra.rs swap_to (PoolState::Algebra, quote/pool_quote.rs и
//! quote/depth_band_algebra.rs), своей записи нет.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/algebra_integral_common, arb::venues::bsc_dex::{math::{swap_algebra, tick_math},
//! pools::ticks, quote::{depth_band, depth_band_algebra}}.

mod algebra_integral_common;

use alloy_primitives::U256;
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState, NextTick, Swapped};
use arb::venues::bsc_dex::math::tick_math::{MIN_TICK, sqrt_ratio_at_tick};
use arb::venues::bsc_dex::pools::ticks::TickMap;
use arb::venues::bsc_dex::quote::depth_band::band_ticks;
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

/// Пул thena-v3 записи: запись пула, цена, тик, ликвидность, комиссия, карта в одном слове вокруг тика, полная карта.
struct Pool<'a> {
    p: &'a Value,
    sqrt: U256,
    tick: i32,
    liq: u128,
    fee: u32,
    m: TickMap,
    full: TickMap,
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
                full: m,
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
        ..
    } in pools(&d)
    {
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee,
            ticks: &m,
            next: NextTick::Linked,
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
        // hi + 1 — нули в обе стороны; lo − 1 — только вниз (вверх считается — tick_below_lower_edge_up_as_full_map).
        for (t, ways) in [(hi + 1, &[true, false][..]), (lo - 1, &[true][..])] {
            let sqrt = sqrt_ratio_at_tick(t).unwrap();
            let s = AlgebraState {
                sqrt_price: sqrt,
                tick: t,
                liquidity: liq,
                fee,
                ticks: &m,
                next: NextTick::Linked,
            };
            for (zfo, amount, _) in small_quotes(p) {
                if !ways.contains(&zfo) {
                    continue;
                }
                let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
                assert_eq!(r, NONE, "{} тик {t} {zfo}: не нули", p["pool"]);
            }
            let s0 = AlgebraState { fee: 0, ..s };
            for &zfo in ways {
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
            next: NextTick::Linked,
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
        ..
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
            next: NextTick::Linked,
        };
        for (zfo, amount, _) in small_quotes(p) {
            let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
            assert_eq!(r, NONE, "{} {zfo}: пустое окно не нули", p["pool"]);
        }
    }
}

/// Состояние пула на карте `map`: цена, тик, ликвидность, комиссия. Ошибок нет.
fn at(map: &TickMap, sqrt: U256, tick: i32, liq: u128, fee: u32) -> AlgebraState<'_> {
    AlgebraState {
        sqrt_price: sqrt,
        tick,
        liquidity: liq,
        fee,
        ticks: map,
        next: NextTick::Linked,
    }
}

#[test]
fn tick_below_lower_edge_up_as_full_map() {
    let d = probe();
    for Pool {
        p,
        tick,
        liq,
        fee,
        m,
        full,
        ..
    } in pools(&d)
    {
        let w = (tick >> 8) as i16;
        let (lo, hi) = (i32::from(w) * 256, i32::from(w) * 256 + 255);
        // Ликвидность отрезка тика lo − 1: Swap вниз от записанного тика пересёк тики [lo; tick] — каждый вычитает свою
        // liquidityDelta (AlgebraPool: zeroToOne вычитает), цена остановилась на цене тика lo − 1 (тик lo − 1 не пересечён).
        let crossed: i128 = full.ticks.range(lo..=tick).map(|(_, x)| x.net).sum();
        let below =
            u128::try_from(i128::try_from(liq).unwrap() - crossed).expect("ликвидность ниже lo");
        let sqrt = sqrt_ratio_at_tick(lo - 1).unwrap();
        let (narrow, wide) = (
            at(&m, sqrt, lo - 1, below, fee),
            at(&full, sqrt, lo - 1, below, fee),
        );
        // Предел — цена тика hi: узкая карта не упирается в свой край раньше полной.
        let limit = sqrt_ratio_at_tick(hi).unwrap();
        for (zfo, amount, _) in small_quotes(p) {
            if zfo {
                continue;
            }
            let (r, _) =
                swap_algebra::swap_to(&narrow, false, true, amount, limit).expect("расчёт узкой");
            let (f, _) =
                swap_algebra::swap_to(&wide, false, true, amount, limit).expect("расчёт полной");
            assert!(
                !r.amount_in.is_zero(),
                "{}: вверх от lo − 1 нули",
                p["pool"]
            );
            assert_eq!(r, f, "{}: вверх от lo − 1 ≠ полная карта", p["pool"]);
        }
        let up = depth_band_algebra::algebra(&AlgebraState { fee: 0, ..narrow }, false, 0.02, true)
            .expect("глубина узкой");
        let up_full =
            depth_band_algebra::algebra(&AlgebraState { fee: 0, ..wide }, false, 0.02, true)
                .expect("глубина полной");
        assert!(up.2, "{}: глубина вверх от lo − 1 не дошла", p["pool"]);
        assert_eq!(up, up_full, "{}: глубина вверх ≠ полная карта", p["pool"]);
    }
}

#[test]
fn tick_below_lower_edge_crosses_recorded_ticks() {
    let d = probe();
    let mut checked = 0;
    for Pool {
        p,
        tick,
        liq,
        fee,
        full,
        ..
    } in pools(&d)
    {
        // Окно — слово ближайшего записанного тика ниже записанного тика пула.
        let below_tick = *full.ticks.range(..tick).next_back().expect("тик ниже").0;
        let w = (below_tick >> 8) as i16;
        let (lo, hi) = (i32::from(w) * 256, i32::from(w) * 256 + 255);
        if lo - 1 < MIN_TICK {
            continue;
        }
        let m = narrowed(&full, (w, w));
        // Swap вниз от записанного тика до цены тика lo − 1 пересёк тики [lo; tick] — каждый вычитает свою liquidityDelta.
        let crossed: Vec<i128> = full.ticks.range(lo..=tick).map(|(_, x)| x.net).collect();
        assert!(
            !crossed.is_empty(),
            "{}: между lo и тиком нет тиков",
            p["pool"]
        );
        let below = u128::try_from(i128::try_from(liq).unwrap() - crossed.iter().sum::<i128>())
            .expect("ликвидность ниже lo");
        let sqrt = sqrt_ratio_at_tick(lo - 1).unwrap();
        let (narrow, wide) = (
            at(&m, sqrt, lo - 1, below, fee),
            at(&full, sqrt, lo - 1, below, fee),
        );
        let limit = sqrt_ratio_at_tick(hi).unwrap();
        for (zfo, amount, _) in small_quotes(p) {
            if zfo {
                continue;
            }
            let (r, _) =
                swap_algebra::swap_to(&narrow, false, true, amount, limit).expect("расчёт узкой");
            let (f, _) =
                swap_algebra::swap_to(&wide, false, true, amount, limit).expect("расчёт полной");
            assert!(
                !r.amount_in.is_zero(),
                "{}: вверх от lo − 1 нули",
                p["pool"]
            );
            assert_eq!(r, f, "{}: вверх от lo − 1 ≠ полная карта", p["pool"]);
        }
        // Глубина 2% вверх (band_ticks — те же тики полосы, что у depth.rs off_map) пересекает записанный тик внутри
        // окна и кончается в окне — предусловие из записи.
        let band = band_ticks(0.02, true).unwrap();
        assert!(lo - 1 + band < hi, "{}: полоса шире окна", p["pool"]);
        assert!(
            full.ticks.range(lo..=lo - 1 + band).next().is_some(),
            "{}: полоса не пересекает тиков",
            p["pool"]
        );
        let up = depth_band_algebra::algebra(&AlgebraState { fee: 0, ..narrow }, false, 0.02, true)
            .expect("глубина узкой");
        let up_full =
            depth_band_algebra::algebra(&AlgebraState { fee: 0, ..wide }, false, 0.02, true)
                .expect("глубина полной");
        assert!(up.2, "{}: глубина вверх от lo − 1 не дошла", p["pool"]);
        assert_eq!(up, up_full, "{}: глубина вверх ≠ полная карта", p["pool"]);
        checked += 1;
    }
    assert!(checked > 0, "в записи нет пула для случая");
}
