//! Файл: tests/bsc_dex_depth_band.rs
//! Что делает: решение 73 — глубина CL-пулов по направлениям в полосе цены на живой записи 2026-10-25
//! (tests/data/depth_2026-10-25/responses.json; у каждого пула свой блок):
//! - пулы USDT монет ZKP, OPN, IOST (Infinity CL и Uniswap V4, в том числе tickSpacing 1): глубина покупки и продажи
//!   в полосе считается, полоса прочитана целиком, шире полоса — не меньше глубина; если внутри полосы нет
//!   установленных тиков — глубина совпадает с формулой одного диапазона (Δx = L·(√P − √Pₑ)/(√P·√Pₑ),
//!   Δy = L·(√Pₑ − √P), без комиссии пула — чистый объём);
//! - полоса шире прочитанных слов (±50% при tickSpacing 1, слов ±8 — около ±22% цены): full = false, глубина — до
//!   края прочитанной карты; шире полоса — не меньше глубина;
//! - IOST Uniswap V4 0xbccf1b3f… (tickSpacing 18000): без комиссии пула глубины покупки и продажи сопоставимы
//!   (с комиссией они расходились в 10 раз — 24,54 против 2,45 USDT).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::quote::depth_band.

mod depth_common;
mod native_common;

use rust_decimal::Decimal;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::quote::depth_band::band_units;

use depth_common::{cl_entry, closed_form, f, rec, usd};

#[tokio::test]
async fn cl_band_depth_zkp_opn_iost() {
    let r = rec();
    let (mut seen, mut formula) = (0, 0);
    for p in r.pools.iter().filter(|p| {
        matches!(p["coin"].as_str(), Some("ZKP" | "OPN" | "IOST"))
            && matches!(p["kind"].as_str(), Some("infinity_cl" | "uni_v4"))
    }) {
        let Some(e) = cl_entry(&r, p) else { continue };
        let PoolState::Cl {
            liquidity, tick, ..
        } = e.pool.state
        else {
            unreachable!()
        };
        if liquidity == 0 {
            continue;
        }
        seen += 1;
        let tag = p["tag"].as_str().unwrap();
        let (buy, sell, full) = band_units(&e, Decimal::from(2)).unwrap();
        let (buy5, sell5, _) = band_units(&e, Decimal::from(5)).unwrap();
        eprintln!(
            "{tag}: tickSpacing {}, ±2%: покупка {} USDT, продажа {} USDT; ±5%: {} / {}",
            p["spacing"],
            usd(buy).round_dp(2),
            usd(sell).round_dp(2),
            usd(buy5).round_dp(2),
            usd(sell5).round_dp(2)
        );
        assert!(full, "{tag}: полоса ±2% внутри прочитанных слов");
        assert!(
            buy5 >= buy && sell5 >= sell,
            "{tag}: шире полоса — не меньше глубина"
        );
        // Внутри ±2% нет установленных тиков — одна ликвидность, формула сходится.
        let ticks = &e.ticks.as_ref().unwrap().ticks;
        let (lo, hi) = (tick - 210, tick + 210);
        if ticks.range(lo..=hi).next().is_none() {
            let (b, s) = closed_form(&e, 2.0);
            formula += 1;
            let rel = |x: f64, y: f64| ((x - y) / y).abs();
            assert!(
                rel(f(buy), b) < 1e-6,
                "{tag}: покупка {} против формулы {b}",
                f(buy)
            );
            assert!(
                rel(f(sell), s) < 1e-6,
                "{tag}: продажа {} против формулы {s}",
                f(sell)
            );
        }
    }
    assert!(seen >= 5, "пулов USDT с ликвидностью в записи: {seen}");
    assert!(
        formula >= 3,
        "пулов без тиков в полосе (сверка с формулой): {formula}"
    );
}

#[tokio::test]
async fn band_wider_than_read_words_is_not_full() {
    let r = rec();
    let mut seen = 0;
    for p in r
        .pools
        .iter()
        .filter(|p| p["kind"] == "infinity_cl" && p["spacing"] == 1)
    {
        let Some(e) = cl_entry(&r, p) else { continue };
        let PoolState::Cl { liquidity, .. } = e.pool.state else {
            unreachable!()
        };
        if liquidity == 0 {
            continue;
        }
        seen += 1;
        let tag = p["tag"].as_str().unwrap();
        let (buy, sell, full) = band_units(&e, Decimal::from(50)).unwrap();
        assert!(!full, "{tag}: ±50% при tickSpacing 1 шире ±8 слов");
        // Шире полоса — не меньше глубина (обе без комиссии, карта та же).
        let (buy90, sell90, _) = band_units(&e, Decimal::from(90)).unwrap();
        assert!(
            sell <= sell90 && buy <= buy90,
            "{tag}: шире полоса — не меньше глубина"
        );
    }
    assert!(
        seen >= 1,
        "пулов Infinity CL с tickSpacing 1 и ликвидностью: {seen}"
    );
}

#[tokio::test]
async fn iost_v4_depth_without_fee_is_symmetric() {
    let r = rec();
    let p = r
        .pools
        .iter()
        .find(|p| p["id"] == "0xbccf1b3f0ce8a16511499b4a280e112ab0052f7a321a17ecad89f41feb945219")
        .unwrap();
    let e = cl_entry(&r, p).unwrap();
    let (buy, sell, full) = band_units(&e, Decimal::from(2)).unwrap();
    let (b, s) = (f(buy), f(sell));
    eprintln!(
        "IOST V4 0xbccf1b3f…: ±2% без комиссии — покупка {} USDT, продажа {} USDT",
        usd(buy).round_dp(2),
        usd(sell).round_dp(2)
    );
    assert!(full);
    let ratio = b / s;
    assert!((0.9..1.1).contains(&ratio), "покупка / продажа = {ratio}");
}
