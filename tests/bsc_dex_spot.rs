//! Файл: tests/bsc_dex_spot.rs
//! Что делает: решение 73 (ревью #63) — монеты для пробы продажи при сверке с квотером считаются по ТЕКУЩЕЙ цене пула
//! (quote/spot.rs), без обмена на стороне покупки. На живой записи 2026-10-25:
//! - пулы USDT монет ZKP / OPN / IOST (Infinity CL, Uniswap V4) и AAVE Uniswap V3: монет = сумма / цена, цена —
//!   (sqrtPriceX96 / 2^96)² (токен0 в токене1), совпадает с независимым расчётом в f64;
//! - пара V2 (IOST 0x5fef…): монет = сумма · резерв монеты / резерв встречного (обе ориентации).
//!
//! Пула с ликвидностью только с одной стороны цены в записи нет (самый перекошенный — IOST Infinity CL 0x0ca12d…:
//! без комиссии ±2% — покупка 0,0185, продажа 0,0025 USDT, обе стороны есть); нужна отдельная запись — tests/README.md.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::quote::spot.

mod depth_common;
mod native_common;

use alloy_primitives::U256;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::quote::spot::coins_for;

use depth_common::{aave_v3, cl_entry, f, rec, v2};

/// 1 USDT (18 знаков в BSC).
fn one_usdt() -> U256 {
    U256::from(10u64).pow(U256::from(18u8))
}

/// Ожидаемые монеты на 1 USDT по sqrtPriceX96 (f64). Ошибок нет.
fn by_sqrt(e: &arb::venues::bsc_dex::pools::registry::Entry) -> f64 {
    let s = match e.pool.state {
        PoolState::V3 { sqrt_price_x96, .. } | PoolState::Cl { sqrt_price_x96, .. } => {
            sqrt_price_x96
        }
        _ => unreachable!(),
    };
    let p = (f(s) / 2f64.powi(96)).powi(2);
    let a = f(one_usdt());
    if e.coin == e.pool.token0 {
        a / p
    } else {
        a * p
    }
}

#[test]
fn coins_by_current_price() {
    let r = rec();
    let rel = |x: f64, y: f64| ((x - y) / y).abs();
    let mut seen = 0;
    for p in r
        .pools
        .iter()
        .filter(|p| matches!(p["kind"].as_str(), Some("infinity_cl" | "uni_v4")))
    {
        let Some(e) = cl_entry(&r, p) else { continue };
        let PoolState::Cl { liquidity, .. } = e.pool.state else {
            unreachable!()
        };
        if liquidity == 0 {
            continue;
        }
        let got = coins_for(&e, one_usdt()).unwrap();
        assert!(rel(f(got), by_sqrt(&e)) < 1e-9, "{}", p["tag"]);
        seen += 1;
    }
    assert!(seen >= 5);
    let (aave, _, _) = aave_v3(&r);
    assert!(rel(f(coins_for(&aave, one_usdt()).unwrap()), by_sqrt(&aave)) < 1e-9);
    for coin0 in [true, false] {
        let e = v2(&r, "0x5fef5bcc62164f8fb0d53d58733cc3940ddfc46c", coin0);
        let PoolState::V2 { reserve0, reserve1 } = e.pool.state else {
            unreachable!()
        };
        let (rc, ru) = if coin0 {
            (reserve0, reserve1)
        } else {
            (reserve1, reserve0)
        };
        let want = f(one_usdt()) * f(rc) / f(ru);
        assert!(rel(f(coins_for(&e, one_usdt()).unwrap()), want) < 1e-9);
    }
}
