//! Файл: tests/hook_ticks_common/mod.rs
//! Что делает: чтение досъёмки узла 17.11 (tests/data/hook_pools_ticks_2026-11-17/raw_rpc.json, блок 134000292):
//! пулы Infinity CL (ключ, slot0, liquidity, слова битмапа, данные тиков — разбор node/call.rs decode по ABI
//! CLPoolManager), вызовы CLQuoter, карта тиков записи и наш полный обмен math/swap_cl.rs.
//! Кто вызывает: tests/bsc_dex_hook_pools_ticks_1117.rs, tests/bsc_dex_hook_ladder_1117.rs (каждому нужна своя часть —
//! allow(dead_code)).
//! От чего зависит: arb::venues::bsc_dex::{math::swap_cl, node::call, pools::{abi_v4, ticks}, quote::pool_quote}.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Swapped, Version};
use arb::venues::bsc_dex::node::call;
use arb::venues::bsc_dex::pools::abi_v4::IInfinityCl;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::pool_quote::swap_fee;

pub const DIR: &str = "tests/data/hook_pools_ticks_2026-11-17";
/// Блоки записи (README данных).
pub const H: u64 = 134_000_292;
/// Селекторы (первые 4 байта calldata в записи).
const KEY: &str = "0x0e2d484a";
const SLOT0: &str = "0xc815641c";
const LIQ: &str = "0xfa6793d5";
const WORD: &str = "0x7c352ef6";
const TICK: &str = "0x5aa208a4";
pub const QUOTE: &str = "0x9938b8ed";

pub fn load() -> Vec<Value> {
    let s = std::fs::read_to_string(format!("{DIR}/raw_rpc.json")).expect("запись");
    serde_json::from_str(&s).expect("json")
}

pub fn hex(s: &str) -> Bytes {
    s.parse().expect("hex")
}

pub fn word(h: &str, i: usize) -> U256 {
    let h = h.strip_prefix("0x").unwrap_or(h);
    U256::from_str_radix(&h[64 * i..64 * i + 64], 16).expect("слово")
}

pub fn block(v: &Value) -> u64 {
    u64::from_str_radix(
        v["params"][1]
            .as_str()
            .expect("блок")
            .trim_start_matches("0x"),
        16,
    )
    .expect("блок hex")
}

pub fn data(v: &Value) -> &str {
    v["params"][0]["data"].as_str().expect("calldata")
}

/// Пул на блоке H: ключ (слова), состояние, слова битмапа, данные тиков.
#[derive(Default)]
pub struct Pool {
    pub key: String,
    pub usdt0: bool,
    pub sqrt: U256,
    pub tick: i32,
    pub pf: u32,
    pub lp: u32,
    pub liq: u128,
    pub words: BTreeMap<i16, U256>,
    pub ticks: BTreeMap<i32, TickData>,
}

/// Знаковое число из младших `bits` бит слова ABI (int16 / int24 — дополнение до двух). Ошибок нет.
pub fn signed(w: U256, bits: usize) -> i64 {
    let mask = U256::from((1u64 << bits) - 1);
    let v = i64::try_from((w & mask).to::<u64>()).expect("в i64");
    let top = 1i64 << (bits - 1);
    if v >= top { v - (top << 1) } else { v }
}

/// Пулы на блоке H по PoolId. Паника — запись не по виду.
pub fn pools(all: &[Value]) -> BTreeMap<String, Pool> {
    let mut out: BTreeMap<String, Pool> = BTreeMap::new();
    for v in all
        .iter()
        .filter(|v| v["method"] == "eth_call" && block(v) == H)
    {
        let d = data(v);
        let Some(res) = v["result"].as_str() else {
            continue;
        };
        if d.len() < 74 || &d[..10] == QUOTE {
            continue;
        }
        let p = out.entry(format!("0x{}", &d[10..74])).or_default();
        let b = hex(res);
        match &d[..10] {
            KEY => {
                p.key = res[2..2 + 64 * 6].to_string();
                // USDT — currency0 ключа (адрес USDT BSC из записи, README данных).
                p.usdt0 = &res[26..66] == "55d398326f99059ff775485246999027b3197955";
            }
            SLOT0 => {
                let s = call::decode::<IInfinityCl::getSlot0Call>(&b).expect("slot0");
                p.sqrt = U256::from(s.sqrtPriceX96);
                p.tick = s.tick.as_i32();
                p.pf = s.protocolFee.to();
                p.lp = s.lpFee.to();
            }
            LIQ => p.liq = call::decode::<IInfinityCl::getLiquidityCall>(&b).expect("liquidity"),
            WORD => {
                let w = i16::try_from(signed(word(&d[10..], 1), 16)).expect("слово");
                p.words.insert(
                    w,
                    call::decode::<IInfinityCl::getPoolBitmapInfoCall>(&b).expect("битмап"),
                );
            }
            TICK => {
                let t = i32::try_from(signed(word(&d[10..], 1), 24)).expect("тик");
                let i = call::decode::<IInfinityCl::getPoolTickInfoCall>(&b).expect("тик");
                p.ticks.insert(
                    t,
                    TickData {
                        gross: i.liquidityGross,
                        net: i.liquidityNet,
                    },
                );
            }
            _ => {}
        }
    }
    out
}

/// Вызов квотера записи на блоке H: пул (PoolId), направление, вход, выход квотера, покупка ли (вход — USDT), сумма
/// USDT покупки (0 у продажи).
pub struct Quote {
    pub id: String,
    pub z: bool,
    pub amount: U256,
    pub remote: U256,
    pub buy: bool,
    pub usd: u64,
}

/// Вызовы квотера на блоке H. Паника — запись не по виду.
pub fn quotes(all: &[Value], pools: &BTreeMap<String, Pool>) -> Vec<Quote> {
    let by_key: BTreeMap<&str, &str> = pools
        .iter()
        .map(|(id, p)| (p.key.as_str(), id.as_str()))
        .collect();
    let unit = U256::from(10u64).pow(U256::from(18u64));
    all.iter()
        .filter(|v| v["method"] == "eth_call" && block(v) == H && data(v).starts_with(QUOTE))
        .map(|v| {
            let body = &data(v)[10..];
            let id = by_key[&body[64..64 * 7]].to_string();
            let z = !word(body, 7).is_zero();
            let amount = word(body, 8);
            let buy = z == pools[&id].usdt0;
            Quote {
                usd: if buy { (amount / unit).to::<u64>() } else { 0 },
                remote: word(v["result"].as_str().expect("ответ"), 0),
                id,
                z,
                amount,
                buy,
            }
        })
        .collect()
}

/// Карта тиков пула из записи (слова битмапа записи). Ошибок нет; паника — запись не по виду.
pub fn map(p: &Pool) -> TickMap {
    let spacing = i32::try_from(signed(word(&p.key, 5) >> 16, 24)).expect("tickSpacing");
    let first = *p.words.keys().next().expect("слова");
    let last = *p.words.keys().last().expect("слова");
    TickMap {
        spacing,
        ticks: Arc::new(p.ticks.clone()),
        words: (first, last),
    }
}

/// Наш полный обмен (math/swap_cl.rs V4) с lpFee `lp`: точный вход (`exact_in`) или точный выход. Паника — откат.
pub fn ours(p: &Pool, map: &TickMap, z: bool, exact_in: bool, amount: U256, lp: u32) -> Swapped {
    let s = ClState {
        sqrt_price: p.sqrt,
        tick: p.tick,
        liquidity: p.liq,
        fee: swap_fee(p.pf, lp, z),
        ticks: map,
        version: Version::V4,
    };
    swap_cl::swap(&s, z, exact_in, amount).expect("обмен")
}
