//! Файл: tests/bsc_thena_fusion_window_t236.rs
//! Что проверяет (ревью t236, решение 463) на записи своего узла 20.11 (tests/data/bsc_thena_fusion_2026-11-20, 8 пулов
//! THENA Fusion, tickSpacing 60, сжатые слова ±2 вокруг floor(tick / 60) >> 8):
//! 1. Окно чтения: load_algebra::row_range при dex.tick_words = 2 и шаге пула 60 — ровно записанные строки (их и
//!    прочитал бы бот), окно карты (TickMap.words) — те же строки.
//! 2. off_map (pools/depth.rs, ветка Proto::Algebra → off_map_algebra::helps → row_range при spacing карты 60) при
//!    dex.algebra_off_map_reread = да, dex.depth_band_pct = 2: тик записи — false (полоса 2 % ≈ 198 тиков внутри окна
//!    ±2 строк по 15360 тиков); тик у верхнего края окна (полоса выходит за край) — true; первый тик за окном — true;
//!    настройка не задана — false.
//! 3. Глубина (quote/depth_band_algebra::algebra — swap_to до цены границы полосы, комиссия 0, как quote/depth_band.rs)
//!    в полосе 2 % (граница 202 тика вниз / 198 вверх — depth_band::band_ticks) у всех 8 пулов в обе стороны: граница
//!    внутри записанных строк — дошли до неё (reached); swap_to той же суммой до той же границы — тот же итог; без предела
//!    выход не меньше (цена шага округляется в сторону обмена); полоса 1 % не глубже 2 %.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/thena_fusion_common, tests/tick_band_common (ctx, set), tests/native_common (entry),
//! tests/depth_common; arb::venues::bsc_dex::{pools::{depth, load_algebra}, quote::depth_band_algebra, math}.
//! Настройки теста: dex.tick_words = 2, dex.depth_band_pct = 2, dex.algebra_off_map_reread — по случаю.

mod depth_common;
mod native_common;
mod thena_fusion_common;
mod tick_band_common;

use alloy_primitives::U256;
use serde_json::json;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::swap_algebra;
use arb::venues::bsc_dex::math::tick_math::sqrt_ratio_at_tick;
use arb::venues::bsc_dex::pools::depth::off_map;
use arb::venues::bsc_dex::pools::load_algebra::row_range;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::depth_band_algebra;

use thena_fusion_common::{FIRST, gs_fee, map_of, pools, state};
use tick_band_common::{ctx, set};

/// Настройка решения 320.
const FLAG: &str = "dex.algebra_off_map_reread";

/// Запись реестра вида thena_fusion из записанного пула: цена, тик, ликвидность, комиссия, карта по сжатым словам.
/// Ошибок нет (тест падает).
fn entry(p: &serde_json::Value) -> Entry {
    let m = map_of(p);
    let s = state(p, &m, gs_fee(p));
    let (sqrt_price_x96, tick, liquidity, fee) = (s.sqrt_price, s.tick, s.liquidity, s.fee);
    let mut e = native_common::entry("thena_fusion", None);
    e.pool.tick_spacing = Some(m.spacing);
    e.pool.state = PoolState::Algebra {
        sqrt_price_x96,
        tick,
        liquidity,
        fee,
        fee_otz: None,
    };
    e.ticks = Some(m);
    e
}

/// Та же запись после Swap: тик `t` и цена этого тика, карта прежняя. Ошибок нет (тест падает).
fn moved(e: &Entry, t: i32) -> Entry {
    let mut out = e.clone();
    if let PoolState::Algebra {
        sqrt_price_x96,
        tick,
        ..
    } = &mut out.pool.state
    {
        *sqrt_price_x96 = sqrt_ratio_at_tick(t).unwrap();
        *tick = t;
    }
    out
}

#[tokio::test]
async fn window_and_off_map_on_record() {
    let c = ctx("thena-fusion-window").await;
    set(&c, "dex.tick_words", Some(json!(2))).await;
    set(&c, "dex.depth_band_pct", Some(json!(2))).await;
    for p in pools(FIRST, 8) {
        let e = entry(&p);
        let m = e.ticks.clone().unwrap();
        let PoolState::Algebra { tick, .. } = e.pool.state else {
            unreachable!()
        };
        let name = p["pool"].as_str().unwrap();
        assert_eq!(m.spacing, 60, "{name}");
        let win = row_range(&c, tick, m.spacing).unwrap();
        assert_eq!(win, m.words, "{name}: окно = записанные строки");
        let top = (i32::from(m.words.1) * 256 + 255) * m.spacing;
        let (edge, out) = (top - 10, top + m.spacing);
        set(&c, FLAG, None).await;
        let (at_edge, beyond) = (moved(&e, edge), moved(&e, out));
        assert!(!off_map(&c, &beyond), "{name}: настройка не задана");
        set(&c, FLAG, Some(json!(true))).await;
        assert!(!off_map(&c, &e), "{name}: полоса внутри окна");
        assert!(off_map(&c, &at_edge), "{name}: полоса за краем окна");
        assert!(off_map(&c, &beyond), "{name}: тик за окном");
        set(&c, FLAG, None).await;
    }
}

#[test]
fn band_depth_on_record() {
    for p in pools(FIRST, 8) {
        let m = map_of(&p);
        let s = state(&p, &m, 0);
        for zfo in [true, false] {
            let case = format!("{} zfo={zfo}", p["pool"]);
            let (a_in, a_out, reached) = depth_band_algebra::algebra(&s, zfo, 0.02, !zfo).unwrap();
            assert!(reached, "{case}: граница полосы внутри строк");
            let filled = a_in > U256::ZERO && a_out > U256::ZERO;
            assert!(filled, "{case}: глубина не пуста");
            // Та же сумма до той же границы — тот же итог (граница достигнута ровно этой суммой).
            let edge = if zfo { s.tick - 202 } else { s.tick + 198 };
            let limit = sqrt_ratio_at_tick(edge).unwrap();
            let (r, at) = swap_algebra::swap_to(&s, zfo, true, a_in, limit).unwrap();
            let same = (r.amount_in, r.amount_out, at) == (a_in, a_out, true);
            assert!(same, "{case}: граница 202 / 198 тиков");
            // Без предела та же сумма уходит не ближе границы: выход не меньше глубины (шаг округляет цену в сторону
            // обмена — на записи до +2 156 881 wei у 0x2465…584f, Python-копия кода Rust).
            let free = swap_algebra::swap(&s, zfo, true, a_in).unwrap();
            assert!(free.complete && free.amount_out >= a_out, "{case}");
            // Полоса 1 % — не глубже полосы 2 %.
            let (i1, o1, r1) = depth_band_algebra::algebra(&s, zfo, 0.01, !zfo).unwrap();
            assert!(r1 && i1 <= a_in && o1 <= a_out, "{case}: 1 % ≤ 2 %");
        }
    }
}
