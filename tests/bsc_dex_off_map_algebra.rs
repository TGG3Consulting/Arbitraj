//! Файл: tests/bsc_dex_off_map_algebra.rs
//! Что делает: решение 320 — проверка выхода полосы глубины за карту тиков (pools/depth.rs off_map) у пулов Algebra
//! (pools/off_map_algebra.rs) под dex.algebra_off_map_reread:
//! - не задана или «нет» — у Algebra false при любом тике (как до решения 320);
//! - «да» и полоса вне окна — true (THENA Fusion — Algebra V1/V1.9: тик у края окна и тик за окном; Algebra Integral —
//!   только тик за окном);
//! - полоса внутри окна — false; окно уже такое, каким его дало бы перечитывание вокруг тика (полоса глубины шире
//!   окна по настройкам), — false (без перечитывания на каждом Swap);
//! - V3 — off_map один и тот же при заданной и не заданной настройке.
//!   THENA Fusion / Algebra Integral в записи depth_2026-10-25 нет. Карта — пул PancakeSwap V3 AAVE/USDT 0x2e31285a…
//!   с tickSpacing 1 этой записи (tests/tick_band_common aave_ts1, слова ±8): при шаге 1 слово компрессированного
//!   битмапа V3 = tick >> 8 — то же слово, что у сырого битмапа Algebra (TickTable.sol: rowNumber = tick >> 8), поэтому
//!   эта карта — ровно то, что дало бы чтение сырого битмапа по тем же словам. Тик «после Swap» — край окна или первый
//!   тик за ним, считается из записанного тика и окна load_algebra::raw_word_range (не выдуманное значение).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/tick_band_common, tests/depth_common, tests/native_common,
//! arb::venues::bsc_dex::pools::{depth, families, load_algebra}, arb::venues::bsc_dex::math::tick_math.
//! Настройки теста: dex.tick_words = 2 (значение сервера 15.11), dex.depth_band_pct = 2 (сервер 15.11) или 10 (полоса
//! шире окна), dex.algebra_off_map_reread — по случаю.

mod depth_common;
mod native_common;
mod tick_band_common;

use serde_json::json;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::tick_math::sqrt_ratio_at_tick;
use arb::venues::bsc_dex::pools::depth::off_map;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::load_algebra::raw_word_range;
use arb::venues::bsc_dex::pools::registry::Entry;

use depth_common::rec;
use tick_band_common::{aave_ts1, ctx, narrowed, set};

/// Настройка решения 320.
const FLAG: &str = "dex.algebra_off_map_reread";

/// Та же запись видом Algebra (`kind` — dex.kind: thena_fusion или algebra_integral): цена, тик, ликвидность и карта —
/// записанные. Ошибок нет (тест падает).
fn algebra(e: &Entry, kind: &str) -> Entry {
    let mut out = e.clone();
    out.family = families::make(kind, kind).unwrap();
    let PoolState::V3 {
        sqrt_price_x96,
        liquidity,
        tick,
    } = e.pool.state
    else {
        panic!("запись не V3");
    };
    out.pool.state = PoolState::Algebra {
        sqrt_price_x96,
        tick,
        liquidity,
        fee: e.pool.fee_pips.unwrap(),
    };
    out
}

/// Текущий тик записи (V3 или Algebra). Ошибок нет (тест падает).
fn tick_of(e: &Entry) -> i32 {
    match e.pool.state {
        PoolState::V3 { tick, .. } | PoolState::Algebra { tick, .. } => tick,
        _ => panic!("не V3 и не Algebra"),
    }
}

/// Запись после Swap: тик `t` и цена этого тика (TickMath), карта прежняя. Ошибок нет (тест падает).
fn moved(e: &Entry, t: i32) -> Entry {
    let mut out = e.clone();
    let p = sqrt_ratio_at_tick(t).unwrap();
    match &mut out.pool.state {
        PoolState::V3 {
            sqrt_price_x96,
            tick,
            ..
        }
        | PoolState::Algebra {
            sqrt_price_x96,
            tick,
            ..
        } => {
            *sqrt_price_x96 = p;
            *tick = t;
        }
        _ => panic!("не V3 и не Algebra"),
    }
    out
}

/// off_map записи при каждом значении настройки: (не задана, нет, да). Ошибок нет (тест падает).
async fn by_flag(c: &Ctx, e: &Entry) -> (bool, bool, bool) {
    set(c, FLAG, None).await;
    let unset = off_map(c, e);
    set(c, FLAG, Some(json!(false))).await;
    let no = off_map(c, e);
    set(c, FLAG, Some(json!(true))).await;
    let yes = off_map(c, e);
    set(c, FLAG, None).await;
    (unset, no, yes)
}

#[tokio::test]
async fn algebra_off_map_by_setting() {
    let r = rec();
    let base = aave_ts1(&r);
    let c = ctx("off-map-algebra").await;
    set(&c, "dex.tick_words", Some(json!(2))).await;
    set(&c, "dex.depth_band_pct", Some(json!(2))).await;
    let t0 = tick_of(&base);
    // Окно, которое прочитал бы load_algebra вокруг записанного тика (±2 слова при dex.tick_words = 2).
    let win = raw_word_range(&c, t0).unwrap();
    let v3 = narrowed(&base, win);
    let top = i32::from(win.1) * 256 + 255;
    // Тик у верхнего края окна (полоса 2% ≈ 198 тиков вверх выходит за край) и первый тик за окном.
    let (edge, out) = (top - 10, top + 1);
    for kind in ["thena_fusion", "algebra_integral"] {
        let a = algebra(&v3, kind);
        assert_eq!(
            by_flag(&c, &a).await,
            (false, false, false),
            "{kind}: полоса внутри окна"
        );
        assert_eq!(
            by_flag(&c, &moved(&a, out)).await,
            (false, false, true),
            "{kind}: тик за окном"
        );
    }
    // Тик в окне, полоса за краем: V1/V1.9 — окно вокруг нового тика другое (перечитывание сдвинет его) — true;
    // Integral — отрезок списка заранее неизвестен, тик в окне — false (частичный итог, как раньше).
    let fusion = algebra(&v3, "thena_fusion");
    assert_eq!(
        by_flag(&c, &moved(&fusion, edge)).await,
        (false, false, true)
    );
    let integral = algebra(&v3, "algebra_integral");
    assert_eq!(
        by_flag(&c, &moved(&integral, edge)).await,
        (false, false, false)
    );
    // V3 — как раньше при любом значении настройки.
    for t in [t0, edge, out] {
        let (unset, no, yes) = by_flag(&c, &moved(&v3, t)).await;
        assert_eq!((unset, unset), (no, yes), "V3, тик {t}");
    }
    assert!(!off_map(&c, &v3), "V3: полоса внутри окна");
    assert!(off_map(&c, &moved(&v3, out)), "V3: тик за окном");
}

#[tokio::test]
async fn algebra_no_reread_when_window_is_already_centered() {
    let r = rec();
    let base = aave_ts1(&r);
    let c = ctx("off-map-algebra-centered").await;
    set(&c, "dex.tick_words", Some(json!(2))).await;
    // Полоса глубины 10% (≈1053 тика вниз) шире окна ±2 слов вокруг тика: карта прочитана ровно так, как прочитало бы
    // перечитывание вокруг этого тика, — перечитывать нечего (иначе — перечитывание на каждом Swap).
    set(&c, "dex.depth_band_pct", Some(json!(10))).await;
    let t0 = tick_of(&base);
    let fusion = algebra(
        &narrowed(&base, raw_word_range(&c, t0).unwrap()),
        "thena_fusion",
    );
    assert_eq!(by_flag(&c, &fusion).await, (false, false, false));
    // Та же карта у V3 (без условия «перечитывание поможет») — полоса вне окна.
    assert!(off_map(
        &c,
        &narrowed(&base, raw_word_range(&c, t0).unwrap())
    ));
}
