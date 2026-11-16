//! Файл: tests/bsc_dex_nomiswap_stable_depth.rs
//! Что проверяет: решение 304 — глубина пула StableSwap в полосе (quote/stable_depth.rs) детерминирована и не
//! обнуляется ложно. Данные — РЕАЛЬНЫЕ ответы узла BSC на блоке 133650877 (tests/data/nomiswap_stable_2026-11-15):
//! три пары Nomiswap Stable (USDT/USDC 0x1e40…60f1, USDT/BUSD 0x3a66…da87, USDC/BUSD 0xca25…a7ec).
//! 1. Каждая пара, обе стороны, полоса 2 % (как dex.depth_band_pct на боте 15.11.2026): глубина не ноль и одинакова
//!    при повторном расчёте.
//! 2. USDT/USDC (монета USDC, встречный USDT): при сдвиге резерва USDT на k·10^15 wei (k = 0..11) глубина меняется
//!    плавно (разброс ≤ 10^-5 от значения) и не обнуляется. Прежний старт поиска с 1 wei на этих же состояниях давал 0
//!    при k = 0, 1, 3, 5, 6 и ≈ 204928 USDT при остальных (сверено портом той же целочисленной математики) — то самое
//!    чередование «глубокий / 0.00» из журнала 15.11.2026.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::load_nomi, quote::stable_depth, math::stableswap_v1}.

use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::core::types::{PoolState, StableData};
use arb::venues::bsc_dex::math::stableswap_v1::Template;
use arb::venues::bsc_dex::pools::load_nomi;
use arb::venues::bsc_dex::quote::stable_depth::{curve_amp, side};

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/nomiswap_stable_2026-11-15/"
);

/// Полоса 2 % — значение dex.depth_band_pct на боте в журнале 15.11.2026 (параметр теста, не настройка движка).
const BAND: f64 = 0.02;

fn probe() -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}probe.json")).unwrap()).unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Слово `i` сырого ответа.
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

fn num<T: std::str::FromStr>(v: &Value) -> T {
    match v.as_str().unwrap().parse() {
        Ok(x) => x,
        Err(_) => panic!("число"),
    }
}

/// Состояние пары по записанным ответам (тот же сборщик, что у чтения на узле).
fn state_of(p: &Value) -> StableData {
    let r = p["getReserves"].as_str().unwrap();
    let st = load_nomi::state(
        addr(&p["token0"]),
        (addr(&p["token0"]), addr(&p["token1"])),
        (word(r, 0), word(r, 1)),
        num(&p["swapFee"]),
        U256::from_str_radix(p["getA"].as_str().unwrap(), 10).unwrap(),
        (
            num(&p["token0PrecisionMultiplier"]),
            num(&p["token1PrecisionMultiplier"]),
        ),
    )
    .expect("состояние");
    let PoolState::StableSwap(d) = st else {
        panic!("не StableSwap")
    };
    *d
}

/// Глубина стороны так же, как её зовёт quote/depth_band.rs (ветка StableSwap): монета — token1 пары.
fn depth(d: &StableData, buy: bool) -> U256 {
    let t = Template {
        amp_precision: d.amp_precision,
        max_rounds: d.max_rounds,
        strict: d.strict,
    };
    let (ic, iu) = (d.i1 as usize, d.i0 as usize);
    side(&d.balances, &d.rates, curve_amp(d), (ic, iu), BAND, buy, &t)
        .expect("расчёт глубины")
        .0
}

#[test]
fn depth_nonzero_and_repeatable_all_pairs() {
    for p in probe()["pairs"].as_array().unwrap() {
        let d = state_of(p);
        for buy in [true, false] {
            let a = depth(&d, buy);
            assert!(!a.is_zero(), "глубина 0: пара {} buy={buy}", p["pair"]);
            assert_eq!(a, depth(&d, buy), "повтор другой: пара {}", p["pair"]);
        }
    }
}

#[test]
fn depth_smooth_under_wei_reserve_shifts() {
    let base = state_of(&probe()["pairs"][0]);
    let step = U256::from(10u64).pow(U256::from(15u8));
    for buy in [true, false] {
        let mut got = Vec::new();
        for k in 0..12u64 {
            let mut d = base.clone();
            d.balances[0] += step * U256::from(k);
            let a = depth(&d, buy);
            assert!(!a.is_zero(), "глубина 0 при k={k} buy={buy}");
            assert_eq!(a, depth(&d, buy));
            got.push(a);
        }
        let (lo, hi) = (*got.iter().min().unwrap(), *got.iter().max().unwrap());
        assert!(
            (hi - lo) * U256::from(100_000u64) <= lo,
            "скачет: {lo}..{hi}"
        );
    }
    // Порядок величины покупки на блоке: ≈ 204927.8 USDT (порт целочисленной математики) — допуск 1 USDT.
    let a = depth(&base, true);
    let want = U256::from_str_radix("204927836502650994299968", 10).unwrap();
    let diff = if a > want { a - want } else { want - a };
    assert!(
        diff < U256::from(10u64).pow(U256::from(18u8)),
        "покупка {a}"
    );
}
