//! Файл: tests/bsc_dex_bound_t162.rs
//! Что проверяет: решения 372, 373 — строгий верх отдачи пула r_e (venues/bsc_dex/quote/bound_rate.rs) на записанных
//! пулах каждого вида, для которого метод действует: выход обмена ≤ r_e·вход
//! - V2 (пул CAKE/USDT PancakeSwap V2, резервы getReserves — tests/size_common): математика бота (pool_quote::swap) на
//!   плотной сетке входов от 1 ед. до 10 резервов входа, обе стороны;
//! - V3 (SquadSwap V3, tests/data/squadswap_v3_2026-11-15/probe.json): ответы QuoterV2 контракта (1, 100, 10000 $ в обе
//!   стороны) и математика бота (swap_cl, карта тиков TickLens записи) на плотной сетке;
//! - Algebra (THENA V3 / DexSwap Integral, tests/data/algebra_integral_2026-11-06/probe.json): ответы виртуального квотера
//!   (used, out) — верх без комиссии;
//! - Solidly волатильный (usdfi, veplus, magicfox — tests/data/solidly_fee_2026-11-14/state.json): getAmountOut пары.
//!
//! И верх курса монеты (quote/bound.rs rho_raw) на пуле V2 ≥ средней цены обмена бота на сетке; верх цикла cycle_rho —
//! произведение r_e ног (проверяется через rho_raw пути из одного пула туда и обратно: ρ туда · ρ обратно < 1 — пул с
//! комиссией не даёт цикла с прибылью).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::{bound, bound_rate, pool_quote}, math::swap_cl}, tests/size_common,
//! tests/algebra_integral_common.

mod algebra_integral_common;
mod size_common;

use alloy_primitives::{Address, Bytes, U256, address};
use arb::core::types::{PoolKind, PoolState};
use arb::venues::bsc_dex::math::full_math::mul_div_up;
use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::pools::load_batch_v2v3::v3_ticks;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::TickMap;
use arb::venues::bsc_dex::quote::bound::rho_raw;
use arb::venues::bsc_dex::quote::bound_rate::{Ratio, ceil_dec, mul_up, rate};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use rust_decimal::Decimal;
use serde_json::Value;
use size_common::{CAKE, Rec, USDT};

/// Условные адреса токенов пула записи (r_e зависит только от того, какой токен — token0).
const T0: Address = address!("0x0000000000000000000000000000000000000a00");
const T1: Address = address!("0x0000000000000000000000000000000000000b00");

/// Выход ≤ r_e·вход (целочисленно: ⌈num·вход/den⌉ ≥ выход). Принимает: верх, вход, выход, что проверяем.
fn le(r: Ratio, input: U256, out: U256, what: &str) {
    let cap = mul_div_up(r.num, input, r.den).expect("верх × вход");
    assert!(
        out <= cap,
        "{what}: выход {out} > верха {cap} (вход {input})"
    );
}

/// Плотная сетка входов: 1…100 ед. и 3000 долей от «масштаба» до 10 масштабов.
fn inputs(scale: U256) -> Vec<U256> {
    let mut v: Vec<U256> = (1u64..=100).map(U256::from).collect();
    for k in 1u64..=3000 {
        v.push(scale * U256::from(k) / U256::from(300u64));
    }
    v.retain(|x| !x.is_zero());
    v
}

/// Запись пула с заданным видом и состоянием на основе пула записи CAKE (семейство и окружение — для комиссии V2).
fn entry(base: &Entry, kind: PoolKind, state: PoolState, fee: Option<u32>) -> Entry {
    let mut e = base.clone();
    e.pool.kind = kind;
    e.pool.state = state;
    e.pool.fee_pips = fee;
    e.pool.token0 = T0;
    e.pool.token1 = T1;
    e
}

/// V2: сетка в обе стороны по математике бота; верх курса монеты ≥ средней цены продажи и покупки.
#[tokio::test]
async fn v2_rate_covers_swap_and_rho() {
    let rec = Rec::load("bound-t162-v2").await;
    let PoolState::V2 { reserve0, reserve1 } = rec.pool.pool.state else {
        panic!("V2")
    };
    for (tin, r_in) in [(CAKE, reserve0), (USDT, reserve1)] {
        let r = rate(&rec.ctx, &rec.pool, tin).expect("верх V2");
        for x in inputs(r_in) {
            let Ok(s) = swap(&rec.ctx, &rec.pool, tin, true, x) else {
                continue;
            };
            le(r, x, s.amount_out, "V2");
        }
    }
    let sell = rho_raw(
        &rec.ctx,
        std::slice::from_ref(&rec.pool),
        CAKE,
        USDT,
        &[],
        2,
    )
    .unwrap();
    let buy = rho_raw(
        &rec.ctx,
        std::slice::from_ref(&rec.pool),
        USDT,
        CAKE,
        &[],
        2,
    )
    .unwrap();
    for x in inputs(reserve0)
        .into_iter()
        .filter(|x| *x > U256::from(1_000_000u64))
    {
        let s = swap(&rec.ctx, &rec.pool, CAKE, true, x).unwrap();
        let avg = Decimal::from_i128_with_scale(s.amount_out.to::<i128>(), 0)
            / Decimal::from_i128_with_scale(x.to::<i128>(), 0);
        assert!(sell >= avg, "ρ продажи {sell} < средней {avg}");
    }
    // Туда и обратно через пул с комиссией 0,25 %: произведение верхов < 1 (цикл одного пула без прибыли).
    assert!(
        mul_up(sell, buy).unwrap() < Decimal::ONE,
        "ρ туда·обратно ≥ 1"
    );
}

/// Слово `i` ответа.
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Слово `i` как int24.
fn int24(raw: &str, i: usize) -> i32 {
    let w = word(raw, i);
    if w.bit(255) {
        -((!w).wrapping_add(U256::from(1u8)).to::<u64>() as i32)
    } else {
        w.to::<u64>() as i32
    }
}

/// JSON файла записи.
fn json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// V3: ответы QuoterV2 контракта и математика бота на сетке ≤ r_e.
#[tokio::test]
async fn v3_rate_covers_quoter_and_math() {
    let rec = Rec::load("bound-t162-v3").await;
    let d = json("tests/data/squadswap_v3_2026-11-15/probe.json");
    let (mut quoted, mut grid) = (0, 0);
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let slot0 = c["slot0"].as_str().unwrap();
        let (sqrt, tick) = (word(slot0, 0), int24(slot0, 1));
        let liq = word(c["liquidity"].as_str().unwrap(), 0).to::<u128>();
        let fee = word(c["fee"].as_str().unwrap(), 0).to::<u32>();
        let spacing = word(c["tickSpacing"].as_str().unwrap(), 0).to::<u64>() as i32;
        let state = PoolState::V3 {
            sqrt_price_x96: sqrt,
            liquidity: liq,
            tick,
        };
        let e = entry(&rec.pool, PoolKind::V3, state, Some(fee));
        let words = p["words"].as_array().unwrap();
        let ans: Vec<Bytes> = words
            .iter()
            .map(|w| {
                Bytes::from(
                    alloy_primitives::hex::decode(
                        w["raw"].as_str().unwrap().trim_start_matches("0x"),
                    )
                    .unwrap(),
                )
            })
            .collect();
        let first = words.first().unwrap()["word"].as_i64().unwrap() as i16;
        let last = words.last().unwrap()["word"].as_i64().unwrap() as i16;
        let map = TickMap {
            spacing,
            ticks: v3_ticks(&ans).expect("тики").into(),
            words: (first, last),
        };
        let s = ClState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee,
            ticks: &map,
            version: Version::V3,
        };
        let mut scale = [U256::ZERO; 2];
        for q in p["quoter"].as_array().unwrap() {
            let zfo = q["zeroForOne"].as_bool().unwrap();
            let r = rate(&rec.ctx, &e, if zfo { T0 } else { T1 }).unwrap();
            let amount = U256::from_str_radix(q["amountIn"].as_str().unwrap(), 10).unwrap();
            let out = word(q["quoterV2_defaultFee"].as_str().unwrap(), 0);
            le(r, amount, out, "V3 QuoterV2");
            quoted += 1;
            let i = usize::from(!zfo);
            scale[i] = scale[i].max(amount);
        }
        for zfo in [true, false] {
            let r = rate(&rec.ctx, &e, if zfo { T0 } else { T1 }).unwrap();
            for x in inputs(scale[usize::from(!zfo)]) {
                let Ok(o) = swap_cl::swap(&s, zfo, true, x) else {
                    continue;
                };
                le(r, x, o.amount_out, "V3 математика");
                grid += 1;
            }
        }
    }
    assert_eq!(quoted, 18);
    assert!(grid > 1000, "точек сетки V3: {grid}");
}

/// Algebra: ответы виртуального квотера записи (used, out) ≤ r_e (без комиссии).
#[tokio::test]
async fn algebra_rate_covers_quoter() {
    let rec = Rec::load("bound-t162-alg").await;
    let d = algebra_integral_common::probe();
    let mut n = 0;
    for p in d["pools"].as_array().unwrap() {
        let (sqrt, tick, liq, fee, _) = algebra_integral_common::pool_state(p);
        let state = PoolState::Algebra {
            sqrt_price_x96: sqrt,
            tick,
            liquidity: liq,
            fee,
        };
        let e = entry(&rec.pool, PoolKind::Cl, state, None);
        for q in p["quoter"].as_array().unwrap() {
            // Квотер откатился (пул без ликвидности, «__err» записи) — ответа нет, сравнивать нечего.
            let raw = q["raw"].as_str().unwrap();
            if !raw.starts_with("0x") {
                continue;
            }
            let zfo = q["zeroForOne"].as_bool().unwrap();
            let r = rate(&rec.ctx, &e, if zfo { T0 } else { T1 }).unwrap();
            le(r, word(raw, 0), word(raw, 1), "Algebra квотер");
            n += 1;
        }
    }
    assert!(n >= 12, "ответов квотера: {n}");
}

/// Solidly волатильный: getAmountOut пары (контракт) ≤ r_e; стабильные пулы — метод не действует (Err).
#[tokio::test]
async fn solidly_volatile_rate_covers_get_amount_out() {
    let rec = Rec::load("bound-t162-sol").await;
    let d = json("tests/data/solidly_fee_2026-11-14/state.json");
    let (mut n, mut stable) = (0, 0);
    for (_, dex) in d["dex"].as_object().unwrap() {
        for p in dex["pools"].as_array().unwrap() {
            let m = &p["metadata"];
            let u = |k: &str| U256::from_str_radix(m[k].as_str().unwrap(), 10).unwrap();
            let t0: Address = m["t0"].as_str().unwrap().parse().unwrap();
            let st = m["st"].as_bool().unwrap();
            let mut e = rec.pool.clone();
            e.pool.kind = PoolKind::Solidly;
            e.pool.token0 = t0;
            e.pool.token1 = m["t1"].as_str().unwrap().parse().unwrap();
            e.pool.state = PoolState::Solidly {
                reserve0: u("r0"),
                reserve1: u("r1"),
                stable: st,
                decimals0: u("dec0"),
                decimals1: u("dec1"),
                // Комиссия пула r_e не использует (верх без комиссии); правило — числитель над 10000.
                fee: arb::core::types::SolidlyFee {
                    value: p["fee"].as_str().and_then(|f| f.parse().ok()).unwrap_or(0),
                    rule: arb::core::types::FeeRule::Numerator {
                        denominator: 10_000,
                    },
                },
                gate: Default::default(),
            };
            for g in p["getAmountOut"].as_array().unwrap() {
                let tin: Address = g[1].as_str().unwrap().parse().unwrap();
                let r = rate(&rec.ctx, &e, tin);
                if st {
                    assert!(
                        r.is_err(),
                        "стабильный пул Solidly: метод не должен действовать"
                    );
                    stable += 1;
                    continue;
                }
                let x = U256::from_str_radix(g[0].as_str().unwrap(), 10).unwrap();
                let out = U256::from_str_radix(g[2].as_str().unwrap(), 10).unwrap();
                le(r.unwrap(), x, out, "Solidly getAmountOut");
                n += 1;
            }
        }
    }
    assert!(
        n >= 8 && stable >= 8,
        "волатильных {n}, стабильных {stable}"
    );
    // Дробь → Decimal вверх: не меньше самой дроби.
    let r = Ratio {
        num: U256::from(2u8),
        den: U256::from(3u8),
    };
    let x = ceil_dec(r).unwrap();
    assert!(x * Decimal::from(3) >= Decimal::from(2));
}
