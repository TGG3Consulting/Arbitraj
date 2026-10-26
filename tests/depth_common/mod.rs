//! Файл: tests/depth_common/mod.rs
//! Что делает: общее для тестов глубины пулов (решение 73): разбор живой записи 2026-10-25
//! (tests/data/depth_2026-10-25/responses.json, рекордер tests/recorders/record_depth.js) в записи реестра бота —
//! CL (Infinity CL, Uniswap V4), AAVE Uniswap V3, пары V2; формула глубины одного диапазона; перевод единиц.
//! Разбор CL — cl.rs, V3 AAVE и V2 — pairs.rs.
//! Кто вызывает: tests/bsc_dex_depth_band.rs, tests/bsc_dex_depth_pairs.rs, tests/bsc_dex_depth_book.rs.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::pools::{abi_v4, abi_v2v3, ticks, registry}.
//! Настройки: нет.
// Каждый тест берёт часть общего — остальное для него не используется.
#![allow(dead_code, unused_imports)]

mod cl;
mod pairs;

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::pools::registry::Entry;

pub use cl::cl_entry;
pub use pairs::{aave_v3, v2};

pub const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

/// Записанные ответы: имя вызова → байты результата.
pub struct Rec {
    pub got: BTreeMap<String, Vec<u8>>,
    pub pools: Vec<Value>,
}

pub fn rec() -> Rec {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/depth_2026-10-25/responses.json").unwrap(),
    )
    .unwrap();
    let mut got = BTreeMap::new();
    for r in v["results"].as_array().unwrap() {
        if let Some(s) = r["response"]["result"].as_str() {
            got.insert(
                r["name"].as_str().unwrap().to_string(),
                alloy_primitives::hex::decode(s).unwrap(),
            );
        }
    }
    Rec {
        got,
        pools: v["pools"].as_array().unwrap().clone(),
    }
}

/// Слово ABI `i` (32 байта) как адрес. Ошибок нет (запись — по форме).
pub fn addr_word(b: &[u8], i: usize) -> Address {
    Address::from_slice(&b[i * 32 + 12..i * 32 + 32])
}

/// Единицы USDT (18 знаков в BSC) → Decimal.
pub fn usd(u: U256) -> Decimal {
    Decimal::from_str(&u.to_string()).unwrap() / Decimal::from(10u64.pow(18))
}

pub fn f(u: U256) -> f64 {
    u.to_string().parse().unwrap()
}

/// Глубина одного диапазона (внутри полосы нет установленных тиков): (покупка — USDT в пул, продажа — USDT из пула)
/// по формулам Δx, Δy, без комиссии пула (глубина — чистый объём пула, решение Тиграна к решению 73). Ошибок нет.
pub fn closed_form(e: &Entry, pct: f64) -> (f64, f64) {
    let (sqrt_price_x96, liquidity, t) = match e.pool.state {
        PoolState::Cl {
            sqrt_price_x96,
            liquidity,
            tick,
            ..
        }
        | PoolState::V3 {
            sqrt_price_x96,
            liquidity,
            tick,
        } => (sqrt_price_x96, liquidity, tick),
        _ => unreachable!(),
    };
    let q = 2f64.powi(96);
    let s = f(sqrt_price_x96) / q;
    let l = liquidity as f64;
    let coin0 = e.coin == e.pool.token0;
    let tick = |up: bool| {
        let b: f64 = pct / 100.0;
        let r = if up { (1.0 + b).ln() } else { -(1.0 - b).ln() };
        (r / 1.0001f64.ln()).floor() as i32
    };
    let at = |k: i32| 1.0001f64.powf(k as f64 / 2.0);
    if coin0 {
        // Монета — токен0: продажа — цена (токен1/токен0) вниз, из пула Δy; покупка — вверх, в пул Δy.
        let down = at(t - tick(false));
        let up = at(t + tick(true));
        (l * (up - s), l * (s - down))
    } else {
        // Монета — токен1: продажа — цена вверх (монеты в пул), из пула Δx USDT; покупка — вниз, в пул Δx.
        let up = at(t + tick(false));
        let down = at(t - tick(true));
        (l * (s - down) / (s * down), l * (up - s) / (s * up))
    }
}
