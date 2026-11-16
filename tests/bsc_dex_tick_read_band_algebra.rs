//! Файл: tests/bsc_dex_tick_read_band_algebra.rs
//! Что делает: решение 319 — отдельная полоса чтения сырого битмапа Algebra dex.tick_read_band_algebra_pct
//! (load_algebra::raw_word_range / raw_band_words) рядом с общей dex.tick_read_band_pct (load_v2v3::word_range):
//! - не задана — окно Algebra ровно прежнее (решение 296) при любой dex.tick_read_band_pct (не задана, 2, 10, 20, 70%):
//!   n = max(dex.tick_words, band_words(dex.tick_read_band_pct, 1));
//! - задана — окно Algebra по ней (шире и уже общей), а окно V3/CL/V4 — по-прежнему по dex.tick_read_band_pct;
//! - снятие на лету — Algebra снова следует dex.tick_read_band_pct.
//!   THENA Fusion в записях нет — функция окна зависит только от числа тика (слово = tick >> 8), берутся текущие тики
//!   и tickSpacing пулов V3 / Infinity CL / Uniswap V4 живой записи tests/data/depth_2026-10-25 (как в
//!   tests/bsc_dex_tick_read_band.rs).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/tick_band_common, tests/depth_common, arb::venues::bsc_dex::pools::{load_v2v3, load_algebra}.
//! Настройки теста: dex.tick_words = 2 (значение сервера 15.11), dex.depth_band_pct = 2, dex.tick_read_band_pct и
//! dex.tick_read_band_algebra_pct — по случаю.

mod depth_common;
mod native_common;
mod tick_band_common;

use rust_decimal::Decimal;
use serde_json::json;

use arb::venues::bsc_dex::pools::load_algebra::raw_word_range;
use arb::venues::bsc_dex::pools::load_v2v3::{band_words, word_range};
use arb::venues::bsc_dex::pools::ticks::TickMap;

use depth_common::rec;
use tick_band_common::{ctx, set, ticked};

/// dex.tick_words сервера 15.11.
const TICK_WORDS: i32 = 2;
/// dex.depth_band_pct сервера 15.11.
const DEPTH_PCT: i64 = 2;

/// Окно [w − n, w + n] в пределах int16. Ошибок нет.
fn around(w: i32, n: i32) -> (i16, i16) {
    (
        (w - n).max(i16::MIN as i32) as i16,
        (w + n).min(i16::MAX as i32) as i16,
    )
}

/// Ожидаемое окно сырого битмапа Algebra при полосе `pct` (None — полосы нет): n = max(dex.tick_words, слов полосы при
/// шаге 1). Ошибок нет.
fn raw_expected(tick: i32, pct: Option<i64>) -> (i16, i16) {
    let n = pct.map_or(TICK_WORDS, |p| {
        TICK_WORDS.max(band_words(Decimal::from(p), 1))
    });
    around(tick >> 8, n)
}

/// Ожидаемое окно V3/CL/V4 (load_v2v3::word_range): n = max(dex.tick_words, слов полосы глубины, слов полосы чтения
/// `read`) при tickSpacing пула. Ошибок нет.
fn v3_expected(tick: i32, spacing: i32, read: Option<i64>) -> (i16, i16) {
    let mut n = TICK_WORDS.max(band_words(Decimal::from(DEPTH_PCT), spacing));
    if let Some(p) = read {
        n = n.max(band_words(Decimal::from(p), spacing));
    }
    around(i32::from(TickMap::word_of(tick, spacing)), n)
}

#[tokio::test]
async fn unset_algebra_band_follows_common_band() {
    let r = rec();
    let pools = ticked(&r);
    assert!(!pools.is_empty(), "в записи нет пулов с картой тиков");
    let c = ctx("algebra-unset").await;
    set(&c, "dex.tick_words", Some(json!(TICK_WORDS))).await;
    set(&c, "dex.depth_band_pct", Some(json!(DEPTH_PCT))).await;
    set(&c, "dex.tick_read_band_algebra_pct", None).await;
    for common in [None, Some(2), Some(10), Some(20), Some(70)] {
        set(&c, "dex.tick_read_band_pct", common.map(|p| json!(p))).await;
        for (tag, _, tick, _) in &pools {
            assert_eq!(
                raw_word_range(&c, *tick).unwrap(),
                raw_expected(*tick, common),
                "{tag}: общая полоса {common:?}"
            );
        }
    }
    // 70% при шаге 1: −ln(0,3)/ln(1,0001) ≈ 12040,3 тика → ⌈…/256⌉ + 1 = 49 слов в сторону (99 вызовов слов на пул).
    let (_, _, tick, _) = &pools[0];
    let raw = raw_word_range(&c, *tick).unwrap();
    assert_eq!((i32::from(raw.1) - i32::from(raw.0)) / 2, 49);
}

#[tokio::test]
async fn set_algebra_band_overrides_only_algebra() {
    let r = rec();
    let pools = ticked(&r);
    let c = ctx("algebra-set").await;
    set(&c, "dex.tick_words", Some(json!(TICK_WORDS))).await;
    set(&c, "dex.depth_band_pct", Some(json!(DEPTH_PCT))).await;
    // (общая, Algebra): уже общей, шире общей, общая не задана.
    for (common, algebra) in [(Some(70), 10), (Some(70), 2), (Some(10), 20), (None, 20)] {
        set(&c, "dex.tick_read_band_pct", common.map(|p| json!(p))).await;
        set(&c, "dex.tick_read_band_algebra_pct", Some(json!(algebra))).await;
        let mut small = 0;
        for (tag, sp, tick, _) in &pools {
            assert_eq!(
                raw_word_range(&c, *tick).unwrap(),
                raw_expected(*tick, Some(algebra)),
                "{tag}: Algebra {algebra}% при общей {common:?}"
            );
            assert_eq!(
                word_range(&c, *tick, *sp).unwrap(),
                v3_expected(*tick, *sp, common),
                "{tag}: V3/CL/V4 при общей {common:?} и Algebra {algebra}%"
            );
            if *sp == 1 {
                small += 1;
            }
        }
        assert!(small >= 2, "пулов tickSpacing 1: {small}");
    }
    // Снятие на лету — Algebra снова по общей полосе.
    set(&c, "dex.tick_read_band_pct", Some(json!(70))).await;
    set(&c, "dex.tick_read_band_algebra_pct", None).await;
    for (tag, _, tick, _) in &pools {
        assert_eq!(
            raw_word_range(&c, *tick).unwrap(),
            raw_expected(*tick, Some(70)),
            "{tag}: после снятия"
        );
    }
}
