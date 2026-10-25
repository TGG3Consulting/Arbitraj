//! Файл: tests/bsc_dex_depth_hysteresis.rs
//! Что делает: решение 73 — гистерезис глубины пула и охват полосы цены словами битмапа:
//! - мелкое направление становится глубоким при глубине ≥ порога, глубокое мелким — только ниже
//!   порога × (1 − dex.depth_hysteresis_pct / 100); у границы вердикт не мигает;
//! - тиков в полосе: вверх ln(1 + b) / ln(1,0001), вниз −ln(1 − b) / ln(1,0001) (TickMath: цена тика 1,0001^tick);
//! - слов битмапа в каждую сторону хватает на полосу при tickSpacing 1 (ZKP, OPN, IOST — Infinity CL, V4) и 10 / 60;
//! - «missing trie node» неархивного узла — своя причина StaleData (ответ из записи глубины 2026-10-25).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::depth_book, quote::depth_band, pools::load_v2v3, errors,
//! node::jsonrpc}.

use rust_decimal::Decimal;
use std::str::FromStr;

use arb::venues::bsc_dex::pools::depth_book::next_deep;
use arb::venues::bsc_dex::pools::load_v2v3::band_words;
use arb::venues::bsc_dex::quote::depth_band::band_ticks;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn hysteresis_holds_verdict_near_threshold() {
    let (min, h) = (d("1000"), d("10"));
    // Первый расчёт и мелкое — по порогу.
    assert!(!next_deep(None, d("999.99"), min, h));
    assert!(next_deep(None, d("1000"), min, h));
    assert!(
        !next_deep(Some(false), d("950"), min, h),
        "мелкое растёт до 950 — ещё мелкое"
    );
    assert!(
        next_deep(Some(false), d("1000"), min, h),
        "дошло до порога — глубокое"
    );
    // Глубокое падает: до 900 (порог × 0,9) — держится, ниже — мелкое.
    assert!(
        next_deep(Some(true), d("950"), min, h),
        "у границы не мигает"
    );
    assert!(next_deep(Some(true), d("900"), min, h));
    assert!(!next_deep(Some(true), d("899.99"), min, h));
    // Колебания 950 ↔ 1010 после того, как стало глубоким, вердикт не меняют.
    let mut deep = Some(next_deep(None, d("1010"), min, h));
    for x in [d("950"), d("1010"), d("905"), d("1001"), d("950")] {
        let now = next_deep(deep, x, min, h);
        assert!(now, "глубина {x}: вердикт не мигает");
        deep = Some(now);
    }
    // Без гистерезиса — строго по порогу.
    assert!(!next_deep(Some(true), d("999"), min, Decimal::ZERO));
}

#[test]
fn band_is_covered_by_bitmap_words() {
    // ±2%: вверх ln(1,02)/ln(1,0001) = 198,03 → 198 тиков, вниз −ln(0,98)/ln(1,0001) = 202,03 → 202.
    assert_eq!(band_ticks(0.02, true), Some(198));
    assert_eq!(band_ticks(0.02, false), Some(202));
    assert_eq!(band_ticks(0.0, true), None);
    assert_eq!(band_ticks(1.0, false), None);
    // tickSpacing 1: 202 тика — меньше слова (256), плюс слово текущего тика — 2 слова в каждую сторону.
    assert_eq!(band_words(d("2"), 1), 2);
    // ±10%: вниз 1053 тика — 5 слов + текущее.
    assert_eq!(band_ticks(0.10, false), Some(1053));
    assert_eq!(band_words(d("10"), 1), 6);
    assert_eq!(band_words(d("10"), 10), 2);
    assert_eq!(band_words(d("10"), 60), 2);
    assert_eq!(
        band_words(d("0"), 1),
        0,
        "полоса не задана как надо — слов сверх dex.tick_words нет"
    );
}

/// Запись глубины 2026-10-25 (bsc-dataseed.bnbchain.org, все вызовы на одном блоке): через ~минуту узел отвечал
/// {"code":-32000,"message":"missing trie node"} — состояние блока удалено. Это своя причина StaleData с объяснением,
/// а не общая ошибка узла.
#[test]
fn missing_trie_node_is_stale_block() {
    use arb::gateway::errors::ErrorCode;
    use arb::venues::bsc_dex::errors::Op;
    use arb::venues::bsc_dex::node::jsonrpc::parse_text;
    let body =
        r#"{"jsonrpc":"2.0","id":173,"error":{"code":-32000,"message":"missing trie node"}}"#;
    let e = parse_text(body, "узел BSC", Op::Read).unwrap_err();
    assert_eq!(e.code, ErrorCode::StaleData);
    assert!(e.message.contains("missing trie node"), "{}", e.message);
    assert!(e.message.contains("узел не архивный"), "{}", e.message);
}
