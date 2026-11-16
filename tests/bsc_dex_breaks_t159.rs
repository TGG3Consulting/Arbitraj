//! Файл: tests/bsc_dex_breaks_t159.rs
//! Что делает: решение 344 (ревью t159 Б4, Н4, Н10) — изломы бинов Liquidity Book V2.1 для подбора объёма
//! (venues/bsc_dex/quote/breaks_bins.rs) на РЕАЛЬНЫХ ответах узла BSC (tests/data/lb_v21_2026-11-15, блок 133716961,
//! пять пулов, все непустые бины; состояние — тем же разборщиком load_lb::decode, что у чтения на узле):
//! - покупка (монета на выходе): изломы — накопленные резервы монеты бинов от активного по направлению обмена;
//! - продажа (монета на входе, с комиссией — Н4): обмен ровно на вход излома k той же математикой, что getSwapOut
//!   (math/lb_swap.rs, сверено с узлом 50 из 50 в tests/bsc_dex_lb_v21_live.rs), отдаёт не больше накопленной
//!   ёмкости первых k бинов: точка излома не перескакивает свой бин (комиссия на опорной волатильности — нижняя
//!   граница комиссии каждого бина обмена, вход без комиссии — вниз; вход излома — не больше нужного).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::breaks_bins, math::lb_swap, pools::{load_lb, bins}}.

use std::collections::BTreeMap;

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::LbBinData;
use arb::venues::bsc_dex::math::lb_swap;
use arb::venues::bsc_dex::pools::bins::BinMap;
use arb::venues::bsc_dex::pools::load_lb;
use arb::venues::bsc_dex::quote::breaks_bins::{BinSide, bins};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/lb_v21_2026-11-15/");

fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

fn u128s(v: &Value) -> u128 {
    v.as_str().unwrap().parse().unwrap()
}

/// Слова ответа eth_call (каждое — записанное десятичное число) → байты ответа (как в tests/bsc_dex_lb_v21_live.rs).
fn words(v: &[&Value]) -> Bytes {
    let mut b = Vec::new();
    for w in v {
        b.extend_from_slice(&dec(w).to_be_bytes::<32>());
    }
    Bytes::from(b)
}

/// Состояние пула разборщиком load_lb::decode по записанным getActiveId, getStaticFeeParameters,
/// getVariableFeeParameters (как в tests/bsc_dex_lb_v21_live.rs).
fn state(probe: &Value, p: &Value) -> LbBinData {
    let s: Vec<&Value> = p["static"].as_array().unwrap().iter().collect();
    let v: Vec<&Value> = p["variable"].as_array().unwrap().iter().collect();
    let answers = vec![words(&[&p["activeId"]]), words(&s), words(&v)];
    let x_is_token0 = addr(&p["tokenX"]) < addr(&p["tokenY"]);
    let step: u32 = p["binStep"].as_str().unwrap().parse().unwrap();
    load_lb::decode(
        &answers,
        x_is_token0,
        step,
        probe["timestamp"].as_u64().unwrap(),
    )
    .unwrap()
}

/// Бины пула (X, Y) — все непустые.
fn bin_map(p: &Value) -> BinMap {
    let mut m = BTreeMap::new();
    for (k, v) in p["bins"].as_object().unwrap() {
        m.insert(k.parse::<u32>().unwrap(), (u128s(&v[0]), u128s(&v[1])));
    }
    BinMap {
        step: p["binStep"].as_str().unwrap().parse().unwrap(),
        reserves: m.into(),
        range: (0, 0xff_ffff),
    }
}

/// Накопленные резервы выходного токена непустых бинов от активного (X — вверх, Y — вниз).
fn capacity(b: &BinMap, active: u32, x_out: bool) -> Vec<U256> {
    let ids: Vec<(u128, u128)> = if x_out {
        b.reserves.range(active..).map(|(_, r)| *r).collect()
    } else {
        b.reserves.range(..=active).rev().map(|(_, r)| *r).collect()
    };
    let mut cum = U256::ZERO;
    let mut out = Vec::new();
    for (rx, ry) in ids {
        let r = U256::from(if x_out { rx } else { ry });
        if r.is_zero() {
            continue;
        }
        cum += r;
        out.push(cum);
    }
    out
}

#[test]
fn lb_breaks_stay_within_their_bins() {
    let probe = json("probe.json");
    let ts = probe["timestamp"].as_u64().unwrap();
    // Сколько изломов на пул — параметр теста.
    let n = 6usize;
    let mut sells = 0;
    for p in probe["pools"].as_array().unwrap() {
        let d = state(&probe, p);
        let b = bin_map(p);
        for x_coin in [true, false] {
            let buy = bins(
                &b,
                d.active_id,
                d.bin_step,
                x_coin,
                false,
                BinSide::Lb(&d),
                U256::MAX,
                n,
            );
            let caps = capacity(&b, d.active_id, x_coin);
            assert!(buy.len() <= n && buy.len() <= caps.len());
            assert_eq!(buy, caps[..buy.len()], "{} покупка", p["pool"]);
            let sell = bins(
                &b,
                d.active_id,
                d.bin_step,
                x_coin,
                true,
                BinSide::Lb(&d),
                U256::MAX,
                n,
            );
            let caps = capacity(&b, d.active_id, !x_coin);
            assert!(sell.windows(2).all(|w| w[0] < w[1]));
            for (k, v) in sell.iter().enumerate() {
                let amount = u128::try_from(*v).unwrap();
                let r = lb_swap::swap_out(&d, &b, ts, x_coin, amount).expect("расчёт");
                assert!(
                    U256::from(r.out) <= caps[k],
                    "{} продажа, излом {k}: выход {} больше ёмкости {}",
                    p["pool"],
                    r.out,
                    caps[k]
                );
                sells += 1;
            }
        }
    }
    assert!(sells > 0, "продажа: изломы есть");
}
