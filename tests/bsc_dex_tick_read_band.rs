//! Файл: tests/bsc_dex_tick_read_band.rs
//! Что делает: решение 296 — дальность чтения карты тиков dex.tick_read_band_pct (load_v2v3::word_range,
//! load_algebra::raw_word_range) на текущих тиках и tickSpacing всех пулов V3 / Infinity CL / Uniswap V4 живой записи
//! 2026-10-25 (tests/data/depth_2026-10-25; tickSpacing 1, 10, 40 … 19997):
//! - настройка не задана — границы ровно прежние: [w − n, w + n], n = max(dex.tick_words, band_words(dex.depth_band_pct))
//!   (полоса глубины не задана и задана 2% — как на сервере 15.11), у сырого битмапа Algebra n = dex.tick_words;
//! - задана — n = max(прежнее n, band_words(полоса чтения, tickSpacing)): при tickSpacing 1 полоса 10% даёт 6 слов,
//!   20% — 10 (было 2), при tickSpacing 50 и крупнее 10% и 20% ничего не меняют; полоса чтения ≤ полосы глубины —
//!   ничего не меняет.
//!   Сырой битмап Algebra (шаг 1): THENA Fusion в записях нет — функция окна зависит только от числа тика, берутся тики
//!   той же записи.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/tick_band_common, tests/depth_common, arb::venues::bsc_dex::pools::{load_v2v3, load_algebra}.
//! Настройки теста: dex.tick_words = 2 (значение сервера 15.11, как в tests/data/fit_volume — README ветки t125),
//! dex.depth_band_pct = 2 или не задана, dex.tick_read_band_pct — по случаю.

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

/// Прежняя формула word_range (до решения 296): n = max(dex.tick_words, слов полосы глубины). Ошибок нет.
fn old_range(tick: i32, spacing: i32, depth: Option<Decimal>) -> (i16, i16) {
    let n = depth.map_or(TICK_WORDS, |p| TICK_WORDS.max(band_words(p, spacing)));
    let w = i32::from(TickMap::word_of(tick, spacing));
    (
        (w - n).max(i16::MIN as i32) as i16,
        (w + n).min(i16::MAX as i32) as i16,
    )
}

/// Половина ширины окна в словах. Ошибок нет.
fn half(r: (i16, i16)) -> i32 {
    (i32::from(r.1) - i32::from(r.0)) / 2
}

#[tokio::test]
async fn unset_read_band_keeps_old_bounds_for_every_spacing() {
    let r = rec();
    let pools = ticked(&r);
    let c = ctx("unset").await;
    set(&c, "dex.tick_words", Some(json!(TICK_WORDS))).await;
    let mut spacings: Vec<i32> = pools.iter().map(|p| p.1).collect();
    spacings.sort();
    spacings.dedup();
    eprintln!("tickSpacing в записи: {spacings:?}");
    for s in [1, 10, 50, 60, 200] {
        assert!(spacings.contains(&s), "в записи нет tickSpacing {s}");
    }
    for pct in [None, Some(2)] {
        set(&c, "dex.depth_band_pct", pct.map(|d| json!(d))).await;
        let depth = pct.map(Decimal::from);
        for (tag, sp, tick, rec_words) in &pools {
            // Тик из ответа лежит в середине записанного окна (рекордер читал ±8 слов вокруг слова тика).
            let w = TickMap::word_of(*tick, *sp);
            assert_eq!(
                i32::from(w),
                (i32::from(rec_words.0) + i32::from(rec_words.1)) / 2,
                "{tag}"
            );
            let got = word_range(&c, *tick, *sp).unwrap();
            assert_eq!(
                got,
                old_range(*tick, *sp, depth),
                "{tag}: полоса глубины {depth:?}"
            );
            assert_eq!(
                half(got),
                TICK_WORDS,
                "{tag}: при 2% и без полосы — ±dex.tick_words"
            );
            let raw = raw_word_range(&c, *tick).unwrap();
            let rw = *tick >> 8;
            assert_eq!(
                raw,
                ((rw - TICK_WORDS) as i16, (rw + TICK_WORDS) as i16),
                "{tag}: сырой битмап"
            );
        }
    }
}

#[tokio::test]
async fn read_band_adds_words_only_for_small_spacing() {
    let r = rec();
    let pools = ticked(&r);
    let c = ctx("set").await;
    set(&c, "dex.tick_words", Some(json!(TICK_WORDS))).await;
    set(&c, "dex.depth_band_pct", Some(json!(2))).await;
    let depth = Some(Decimal::from(2));
    // Полоса чтения не шире полосы глубины — окна прежние.
    set(&c, "dex.tick_read_band_pct", Some(json!(2))).await;
    for (tag, sp, tick, _) in &pools {
        assert_eq!(
            word_range(&c, *tick, *sp).unwrap(),
            old_range(*tick, *sp, depth),
            "{tag}"
        );
    }
    // tickSpacing 1: 10% → 6 слов, 20% → 10 (−ln(0,9)/ln(1,0001) = 1053 тика → ⌈1053/256⌉ + 1; 2231 → ⌈2231/256⌉ + 1).
    for (pct, ts1_words) in [(10, 6), (20, 10)] {
        set(&c, "dex.tick_read_band_pct", Some(json!(pct))).await;
        let p = Decimal::from(pct);
        let (mut small, mut large) = (0, 0);
        for (tag, sp, tick, _) in &pools {
            let got = word_range(&c, *tick, *sp).unwrap();
            let old = old_range(*tick, *sp, depth);
            let w = i32::from(TickMap::word_of(*tick, *sp));
            let n = half(old).max(band_words(p, *sp));
            assert_eq!(got, ((w - n) as i16, (w + n) as i16), "{tag}: {pct}%");
            if *sp == 1 {
                small += 1;
                assert_eq!(half(got), ts1_words, "{tag}: tickSpacing 1, {pct}%");
                assert_eq!(
                    half(got) - half(old),
                    ts1_words - TICK_WORDS,
                    "{tag}: прибавка слов"
                );
            }
            if *sp >= 50 {
                large += 1;
                assert_eq!(
                    got, old,
                    "{tag}: tickSpacing {sp} при {pct}% — без изменений"
                );
            }
            if *sp == 10 {
                assert_eq!(got, old, "{tag}: tickSpacing 10 при {pct}% — без изменений");
            }
            // Сырой битмап Algebra: шаг 1 — те же слова, что у tickSpacing 1.
            let raw = raw_word_range(&c, *tick).unwrap();
            assert_eq!(half(raw), ts1_words, "{tag}: сырой битмап, {pct}%");
        }
        assert!(
            small >= 2 && large >= 10,
            "пулов tickSpacing 1: {small}, ≥ 50: {large}"
        );
    }
    // Снятие настройки на лету — снова прежние окна.
    set(&c, "dex.tick_read_band_pct", None).await;
    for (tag, sp, tick, _) in &pools {
        assert_eq!(
            word_range(&c, *tick, *sp).unwrap(),
            old_range(*tick, *sp, depth),
            "{tag}"
        );
    }
}
