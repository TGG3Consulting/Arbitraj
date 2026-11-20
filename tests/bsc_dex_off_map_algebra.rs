//! Файл: tests/bsc_dex_off_map_algebra.rs
//! Что делает: решение 320 — проверка выхода полосы глубины за карту тиков (pools/depth.rs off_map) у пулов Algebra
//! (pools/off_map_algebra.rs) под dex.algebra_off_map_reread:
//! - не задана или «нет» — у Algebra false при любом тике (как до решения 320);
//! - «да» и полоса вне окна — true (THENA Fusion — Algebra V1/V1.9: тик у края окна и тик за окном; Algebra Integral —
//!   только тик за окном);
//! - полоса внутри окна — false; окно уже такое, каким его дало бы перечитывание вокруг тика (полоса глубины шире
//!   окна по настройкам), — false (без перечитывания на каждом Swap);
//! - V3 — off_map один и тот же при заданной и не заданной настройке;
//! - решение 323 к 320: Algebra Integral, тик ровно на верхнем крае окна hi (обмен вверх — нули,
//!   math/swap_algebra.rs blocked_by_window) — true, пока выше тика в карте меньше dex.integral_tick_steps тиков
//!   (перечитывание расширит прочитанный отрезок); не меньше — false (без перечитывания на каждом Swap); тик hi − 1 —
//!   false; dex.integral_tick_steps не задана — false.
//!
//! THENA Fusion / Algebra Integral в записи depth_2026-10-25 нет. Первые два теста — карта пула PancakeSwap V3 AAVE/USDT
//!   0x2e31285a… с tickSpacing 1 этой записи (tests/tick_band_common aave_ts1, слова ±8): при шаге 1 слово
//!   компрессированного битмапа V3 = tick >> 8 — то же слово, что у сырого битмапа Algebra (TickTable.sol: rowNumber =
//!   tick >> 8), поэтому эта карта — ровно то, что дало бы чтение сырого битмапа по тем же словам. Инициализированных
//!   тиков в этой карте нет (все 17 записанных ответов TickLens, слова 282…298, — пустые списки): эти тесты проверяют
//!   только окно (слова) против полосы, не содержимое карты. Тик «после Swap» — край окна или первый тик за ним,
//!   считается из записанного тика и окна load_algebra::raw_word_range (не выдуманное значение).
//! Третий тест (тик ровно hi у Integral) — настоящая запись Algebra Integral tests/data/algebra_integral_2026-11-06/
//!   probe.json (tests/algebra_integral_common pool_state): пул thena-v3 0x26399a…, обход списка вверх кончился на
//!   записанном тике −94980, окно load_algebra_integral::window — слова до −373 (hi = −95233), выше hi в карте ровно
//!   один записанный тик −94980 (так карту кладёт загрузчик: walk — все прочитанные тики, окно — целые слова).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/tick_band_common, tests/depth_common, tests/native_common, tests/algebra_integral_common,
//! arb::venues::bsc_dex::pools::{depth, families, load_algebra}, arb::venues::bsc_dex::math::tick_math.
//! Настройки теста: dex.tick_words = 2 (значение сервера 15.11; у Integral не участвует), dex.depth_band_pct = 2 (сервер
//! 15.11) или 10 (полоса шире окна), dex.algebra_off_map_reread и dex.integral_tick_steps — по случаю.

mod algebra_integral_common;
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
use arb::venues::bsc_dex::pools::ticks::TickMap;

use algebra_integral_common::{pool_state, probe};
use depth_common::rec;
use tick_band_common::{aave_ts1, ctx, narrowed, set};

/// Настройка решения 320.
const FLAG: &str = "dex.algebra_off_map_reread";
/// Пул thena-v3 (Algebra Integral) записи tests/data/algebra_integral_2026-11-06/probe.json (pools[].pool), у которого
/// обход списка тиков вверх кончился не на MAX_TICK — в карте есть тики выше окна.
const INTEGRAL_POOL: &str = "0x26399af833bca82c0c8c2ef4255b4939b26c39f5";

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
        fee_otz: None,
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

/// Пул Algebra Integral записи probe.json (thena-v3, блок 132036268) записью реестра: globalState (цена, тик), fee(),
/// liquidity(), карта — тики списка, прочитанные обходом вверх и вниз, окно — load_algebra_integral::window по последним
/// прочитанным тикам (tests/algebra_integral_common pool_state — ровно как собирает бот). Ошибок нет (тест падает).
fn integral(p: &serde_json::Value) -> Entry {
    let (sqrt_price_x96, tick, liquidity, fee, map) = pool_state(p);
    let mut e = native_common::entry("algebra_integral", Some(fee));
    e.pool.state = PoolState::Algebra {
        sqrt_price_x96,
        tick,
        liquidity,
        fee,
        fee_otz: None,
    };
    e.ticks = Some(map);
    e
}

#[tokio::test]
async fn integral_reread_on_upper_edge_without_loop() {
    // Пул 0x26399a… (SINGULARRY/WBNB): обход вверх записан на 16 тиков и кончился на −94980 (не MAX_TICK), вниз — до
    // MIN_TICK. Окно window(MIN_TICK, −94980) = слова (−3466; −373), верхний край hi = −373·256 + 255 = −95233; выше hi в
    // карте — ровно один записанный тик −94980 (тот, что обход прочитал за последним полным словом). У второго пула
    // записи (0xd1d3b6…) оба конца списка — MIN_TICK / MAX_TICK, выше окна тиков нет.
    let d = probe();
    let p = d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pool"] == INTEGRAL_POOL)
        .unwrap();
    let kept = integral(p);
    let map = kept.ticks.clone().unwrap();
    let top = i32::from(map.words.1) * 256 + 255;
    let above = map.ticks.range((top + 1)..).count() as u64;
    assert_eq!(
        (map.words.1, top, above),
        (-373, -95233, 1),
        "окно и тики выше окна записи"
    );
    let c = ctx("off-map-algebra-integral-edge").await;
    // Полоса 2% (≈202 тика вниз, 198 вверх) от тика hi выходит за верхний край окна — off_map доходит до integral_helps.
    set(&c, "dex.depth_band_pct", Some(json!(2))).await;
    let edge = moved(&kept, top);
    set(&c, "dex.integral_tick_steps", None).await;
    assert_eq!(
        by_flag(&c, &edge).await,
        (false, false, false),
        "шаги не заданы"
    );
    set(&c, "dex.integral_tick_steps", Some(json!(above + 1))).await;
    assert_eq!(
        by_flag(&c, &edge).await,
        (false, false, true),
        "выше тика меньше шагов — перечитать"
    );
    set(&c, "dex.integral_tick_steps", Some(json!(above))).await;
    assert_eq!(
        by_flag(&c, &edge).await,
        (false, false, false),
        "выше тика не меньше шагов — перечитывание окно не расширит"
    );
    // Та же карта без тиков выше окна (отрезок списка, кончившийся ровно на hi), шаг 1: тик hi — перечитать; тик
    // hi − 1 (обмен в обе стороны) — нет.
    set(&c, "dex.integral_tick_steps", Some(json!(1))).await;
    let mut bare = kept.clone();
    bare.ticks = Some(TickMap {
        ticks: map
            .ticks
            .range(..=top)
            .map(|(t, x)| (*t, *x))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into(),
        ..map
    });
    assert_eq!(by_flag(&c, &moved(&bare, top)).await, (false, false, true));
    assert_eq!(
        by_flag(&c, &moved(&bare, top - 1)).await,
        (false, false, false)
    );
}
