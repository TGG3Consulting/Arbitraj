//! Файл: tests/depth_common/pairs.rs
//! Что делает: пул AAVE Uniswap V3 0x8e3ecc0b… и пары V2 из живой записи глубины 2026-10-25 — записи реестра бота.
//! Кто вызывает: tests/depth_common/mod.rs.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::pools::{abi_v2v3, ticks, registry}.
//! Настройки: нет.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::core::types::{PoolRef, PoolState};
use arb::venues::bsc_dex::pools::abi_v2v3::{ITickLens, IUniswapV3Pool, IV2Pair, IV3Pool};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

use super::super::native_common;
use super::{Rec, USDT};

/// Пул AAVE Uniswap V3 0x8e3ecc0b… из записи как запись реестра бота. Возвращает: (запись, запись пула в
/// responses.json, balanceOf USDT пула на том же блоке).
pub fn aave_v3(r: &Rec) -> (Entry, Value, U256) {
    let p = r
        .pools
        .iter()
        .find(|p| p["addr"] == "0x8e3ecc0b261f1a4db62321090575eb299844f077")
        .unwrap();
    let tag = p["tag"].as_str().unwrap();
    let s = IUniswapV3Pool::slot0Call::abi_decode_returns(&r.got[&format!("{tag} slot0")]).unwrap();
    let liquidity =
        IV3Pool::liquidityCall::abi_decode_returns(&r.got[&format!("{tag} liquidity")]).unwrap();
    let spacing = p["spacing"].as_i64().unwrap() as i32;
    let words = p["words"].as_array().unwrap();
    let words = (
        words[0].as_i64().unwrap() as i16,
        words[1].as_i64().unwrap() as i16,
    );
    let mut ticks = BTreeMap::new();
    for k in words.0..=words.1 {
        let out = &r.got[&format!("{tag} TickLens word {k}")];
        for t in ITickLens::getPopulatedTicksInWordCall::abi_decode_returns(out).unwrap() {
            ticks.insert(
                t.tick.as_i32(),
                TickData {
                    gross: t.liquidityGross,
                    net: t.liquidityNet,
                },
            );
        }
    }
    // Адрес AAVE — calls.json (coins.AAVE).
    let aave = Address::from_str("0xfb6115445bff7b52feb98650c87f44907e58f802").unwrap();
    let usdt = Address::from_str(USDT).unwrap();
    // Uniswap V3: token0 < token1 по адресу.
    let (t0, t1) = if aave < usdt {
        (aave, usdt)
    } else {
        (usdt, aave)
    };
    // fee() пула рекордер не читал: tickSpacing 60 у Uniswap V3 — уровень 3000 (UniswapV3Factory, конструктор:
    // feeAmountTickSpacing[3000] = 60). Комиссия влияет на вход, а проверка — по выходу.
    assert_eq!(spacing, 60);
    let mut e = native_common::entry("uniswap_v3", Some(3000));
    e.pool.token0 = t0;
    e.pool.token1 = t1;
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::V3 {
        sqrt_price_x96: s.sqrtPriceX96.to::<U256>(),
        liquidity,
        tick: s.tick.as_i32(),
    };
    e.ticker = "AAVE".into();
    e.coin = aave;
    e.usdt = usdt;
    e.ticks = Some(TickMap {
        spacing,
        ticks,
        words,
    });
    let bal = U256::from_be_slice(&r.got[&format!("{tag} USDT balanceOf")]);
    (e, p.clone(), bal)
}

/// Пара V2 из записи (getReserves) как запись реестра бота, монета — токен0 (`coin0`) или токен1; встречный — другой
/// токен пары. Какой это токен на самом деле, запись не говорит (USDT у пар AAVE нет — balanceOf 0) — глубина
/// проверяется в единицах встречного токена по формуле пары; комиссия пары 0,25% (PancakeLibrary: 9975/10000) — для
/// котировки, в глубину не входит.
pub fn v2(r: &Rec, addr: &str, coin0: bool) -> Entry {
    let p = r.pools.iter().find(|p| p["addr"] == addr).unwrap();
    let out = &r.got[&format!("{} getReserves", p["tag"].as_str().unwrap())];
    let rs = IV2Pair::getReservesCall::abi_decode_returns(out).unwrap();
    let (a, b) = (
        Address::from_str("0x00000000000000000000000000000000000000a1").unwrap(),
        Address::from_str("0x00000000000000000000000000000000000000b2").unwrap(),
    );
    let mut e = native_common::entry("pancakeswap_v2", Some(2500));
    e.pool.reference = PoolRef::Contract(Address::from_str(addr).unwrap());
    e.pool.token0 = a;
    e.pool.token1 = b;
    e.pool.state = PoolState::V2 {
        reserve0: U256::from(rs.reserve0),
        reserve1: U256::from(rs.reserve1),
    };
    e.coin = if coin0 { a } else { b };
    e.usdt = if coin0 { b } else { a };
    e
}
