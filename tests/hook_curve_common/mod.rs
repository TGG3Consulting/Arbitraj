//! Файл: tests/hook_curve_common/mod.rs
//! Что делает: чтение записи узла 18.11 (tests/data/hook_curve_2026-11-18/raw_rpc.json, блок 134300230): пулы
//! Infinity CL (ключ, slot0, liquidity, слова битмапа ±10, данные тиков — разбор node/call.rs decode по ABI
//! CLPoolManager), вызовы CLQuoter (с откатами — «нет котировки»), карта тиков записи и наш полный обмен
//! math/swap_cl.rs без крючка. Разбор — тот же, что у tests/hook_ticks_common (запись 17.11), но блок, файл и откаты
//! квотера — свои (в записи 17.11 откатов не было; общий модуль с константой блока не трогаю — точечная правка).
//! Кто вызывает: tests/bsc_dex_hook_curve_t190.rs (через points.rs).
//! От чего зависит: arb::venues::bsc_dex::{math::swap_cl, node::call, pools::{abi_v4, ticks}, quote::pool_quote}.

pub mod points;

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Swapped, Version};
use arb::venues::bsc_dex::node::call;
use arb::venues::bsc_dex::pools::abi_v4::IInfinityCl;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::pool_quote::swap_fee;

pub const DIR: &str = "tests/data/hook_curve_2026-11-18";
/// Блок записи (README данных).
pub const H: u64 = 134_300_230;
/// Селекторы (первые 4 байта calldata в записи; те же, что в записи 17.11).
const KEY: &str = "0x0e2d484a";
const SLOT0: &str = "0xc815641c";
const LIQ: &str = "0xfa6793d5";
const WORD: &str = "0x7c352ef6";
const TICK: &str = "0x5aa208a4";
pub const QUOTE: &str = "0x9938b8ed";
/// Адрес USDT BSC в ключах пулов записи (README данных, currency0 / currency1).
const USDT: &str = "55d398326f99059ff775485246999027b3197955";

/// Вся запись. Паника — файла нет или не JSON.
pub fn load() -> Vec<Value> {
    let s = std::fs::read_to_string(format!("{DIR}/raw_rpc.json")).expect("запись");
    serde_json::from_str(&s).expect("json")
}

/// Слово ABI номер i из hex без 0x. Паника — короче.
pub fn word(h: &str, i: usize) -> U256 {
    let h = h.strip_prefix("0x").unwrap_or(h);
    U256::from_str_radix(&h[64 * i..64 * i + 64], 16).expect("слово")
}

/// Блок вызова (второй параметр eth_call). Паника — не по виду.
pub fn block(v: &Value) -> u64 {
    let b = v["params"][1].as_str().expect("блок");
    u64::from_str_radix(b.trim_start_matches("0x"), 16).expect("блок hex")
}

/// calldata вызова. Паника — не по виду.
pub fn data(v: &Value) -> &str {
    v["params"][0]["data"].as_str().expect("calldata")
}

/// Пул на блоке H: ключ (слова), USDT — currency0 ли, состояние, слова битмапа, данные тиков.
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

/// Знаковое число из младших `bits` бит слова ABI (дополнение до двух). Ошибок нет.
fn signed(w: U256, bits: usize) -> i64 {
    let mask = U256::from((1u64 << bits) - 1);
    let v = i64::try_from((w & mask).to::<u64>()).expect("в i64");
    let top = 1i64 << (bits - 1);
    if v >= top { v - (top << 1) } else { v }
}

/// Все eth_call записи на блоке H (паника — вызов на другом блоке: запись не по виду).
pub fn calls(all: &[Value]) -> Vec<&Value> {
    let out: Vec<&Value> = all.iter().filter(|v| v["method"] == "eth_call").collect();
    assert!(out.iter().all(|v| block(v) == H), "все eth_call на блоке H");
    out
}

/// Пулы по PoolId (0x + 64 hex). Паника — запись не по виду.
pub fn pools(all: &[Value]) -> BTreeMap<String, Pool> {
    let mut out: BTreeMap<String, Pool> = BTreeMap::new();
    for v in calls(all) {
        let d = data(v);
        let Some(res) = v["result"].as_str() else {
            continue;
        };
        if d.len() < 74 || &d[..10] == QUOTE {
            continue;
        }
        let p = out.entry(format!("0x{}", &d[10..74])).or_default();
        let b: Bytes = res.parse().expect("hex");
        match &d[..10] {
            KEY => {
                p.key = res[2..2 + 64 * 6].to_string();
                p.usdt0 = &res[26..66] == USDT;
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
                let bits = call::decode::<IInfinityCl::getPoolBitmapInfoCall>(&b).expect("битмап");
                p.words.insert(w, bits);
            }
            TICK => {
                let t = i32::try_from(signed(word(&d[10..], 1), 24)).expect("тик");
                let i = call::decode::<IInfinityCl::getPoolTickInfoCall>(&b).expect("тик");
                let (gross, net) = (i.liquidityGross, i.liquidityNet);
                p.ticks.insert(t, TickData { gross, net });
            }
            _ => {}
        }
    }
    out
}

/// Вызов квотера: пул, направление, вход, выход квотера (None — откат), покупка ли (вход — USDT).
pub struct Quote {
    pub id: String,
    pub z: bool,
    pub amount: U256,
    pub remote: Option<U256>,
    pub buy: bool,
}

/// Вызовы квотера (с откатами). Паника — запись не по виду.
pub fn quotes(all: &[Value], pools: &BTreeMap<String, Pool>) -> Vec<Quote> {
    let by_key: BTreeMap<&str, &str> = pools
        .iter()
        .map(|(id, p)| (p.key.as_str(), id.as_str()))
        .collect();
    calls(all)
        .into_iter()
        .filter(|v| data(v).starts_with(QUOTE))
        .map(|v| {
            let body = &data(v)[10..];
            let id = by_key[&body[64..64 * 7]].to_string();
            let z = !word(body, 7).is_zero();
            Quote {
                buy: z == pools[&id].usdt0,
                remote: v["result"].as_str().map(|r| word(r, 0)),
                amount: word(body, 8),
                id,
                z,
            }
        })
        .collect()
}

/// Карта тиков пула из записи (слова битмапа записи). Паника — запись не по виду.
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

/// Наш полный обмен без крючка (math/swap_cl.rs V4, комиссия slot0) с точным входом. None — откат нашей математики
/// или вход не исполнен целиком в пределах прочитанных слов записи.
pub fn ours(p: &Pool, map: &TickMap, z: bool, amount: U256) -> Option<Swapped> {
    let s = ClState {
        sqrt_price: p.sqrt,
        tick: p.tick,
        liquidity: p.liq,
        fee: swap_fee(p.pf, p.lp, z),
        ticks: map,
        version: Version::V4,
    };
    swap_cl::swap(&s, z, true, amount)
        .ok()
        .filter(|r| r.complete)
}
