//! Файл: tests/bsc_dex_hook_pools_ticks_1117.rs
//! Что проверяет: досъёмка узла 17.11 (tests/data/hook_pools_ticks_2026-11-17, свой узел BSC):
//! 1. Полный обмен НАШИМ кодом (math/swap_cl.rs swap, версия V4, карта тиков из ответов узла: битмап разобран
//!    pools/load_cl.rs set_ticks, данные тиков — node/call.rs decode getPoolTickInfo, комиссия quote/pool_quote.rs
//!    swap_fee) против CLQuoter на 1, 20, 100 USDT в обе стороны для пулов Infinity CL GAIB, BTCMT, CAKE 0xc528 на
//!    блоке 134000292: совпадение до wei везде, кроме покупки GAIB и BTCMT на 20 и 100 USDT — там квотер = наш
//!    обмен до wei с подменой lpFee крючком (lpFeeOverride, зависит от суммы: GAIB 1492 / 7332, BTCMT 8057 / 36193 —
//!    найдено перебором по записи), и покупки CAKE на 20 и 100 USDT — цена уходит за слова, снятые серверным ИИ
//!    (±10 от слова тика), наш расчёт честно неполный (complete = false; бот читает шире — dex.tick_read_band_pct).
//! Пакет Multicall3 против отдельных вызовов — tests/bsc_dex_hook_pools_batch_1117.rs.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::swap_cl, pools::{abi_v4, load_cl, ticks}, node::call,
//! quote::pool_quote}.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use alloy_primitives::{Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::node::call;
use arb::venues::bsc_dex::pools::abi_v4::IInfinityCl;
use arb::venues::bsc_dex::pools::load_cl::set_ticks;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::pool_quote::swap_fee;

const DIR: &str = "tests/data/hook_pools_ticks_2026-11-17";
/// Блоки записи (README данных).
const H: u64 = 134_000_292;
/// Селекторы (первые 4 байта calldata в записи).
const KEY: &str = "0x0e2d484a";
const SLOT0: &str = "0xc815641c";
const LIQ: &str = "0xfa6793d5";
const WORD: &str = "0x7c352ef6";
const TICK: &str = "0x5aa208a4";
const QUOTE: &str = "0x9938b8ed";

/// Подмена lpFee крючком 0x32c59d…, при которой наш обмен = квотеру до wei (перебор по записи): (пул, сумма USDT, lpFee).
const OVERRIDE: [(&str, u64, u32); 4] = [
    ("0x5c1b4d20", 20, 1492),
    ("0x5c1b4d20", 100, 7332),
    ("0x62166b8a", 20, 8057),
    ("0x62166b8a", 100, 36193),
];

fn load() -> Vec<Value> {
    let s = std::fs::read_to_string(format!("{DIR}/raw_rpc.json")).expect("запись");
    serde_json::from_str(&s).expect("json")
}

fn hex(s: &str) -> Bytes {
    s.parse().expect("hex")
}

fn word(h: &str, i: usize) -> U256 {
    let h = h.strip_prefix("0x").unwrap_or(h);
    U256::from_str_radix(&h[64 * i..64 * i + 64], 16).expect("слово")
}

fn block(v: &Value) -> u64 {
    u64::from_str_radix(
        v["params"][1]
            .as_str()
            .expect("блок")
            .trim_start_matches("0x"),
        16,
    )
    .expect("блок hex")
}

fn data(v: &Value) -> &str {
    v["params"][0]["data"].as_str().expect("calldata")
}

/// Пул на блоке H: ключ (слова), состояние, слова битмапа, данные тиков.
#[derive(Default)]
struct Pool {
    key: String,
    usdt0: bool,
    sqrt: U256,
    tick: i32,
    pf: u32,
    lp: u32,
    liq: u128,
    words: BTreeMap<i16, U256>,
    ticks: BTreeMap<i32, TickData>,
}

/// Знаковое число из младших `bits` бит слова ABI (int16 / int24 — дополнение до двух). Ошибок нет.
fn signed(w: U256, bits: usize) -> i64 {
    let mask = U256::from((1u64 << bits) - 1);
    let v = i64::try_from((w & mask).to::<u64>()).expect("в i64");
    let top = 1i64 << (bits - 1);
    if v >= top { v - (top << 1) } else { v }
}

/// Пулы на блоке H по PoolId. Паника — запись не по виду.
fn pools(all: &[Value]) -> BTreeMap<String, Pool> {
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

#[test]
fn full_swap_vs_quoter() {
    let all = load();
    let pools = pools(&all);
    assert_eq!(pools.len(), 3);
    let by_key: HashMap<&str, &str> = pools
        .iter()
        .map(|(id, p)| (p.key.as_str(), id.as_str()))
        .collect();
    let usd = U256::from(10u64).pow(U256::from(18u64));
    let (mut exact, mut over, mut short) = (0, 0, 0);
    for v in all
        .iter()
        .filter(|v| v["method"] == "eth_call" && block(v) == H && data(v).starts_with(QUOTE))
    {
        let body = &data(v)[10..];
        let id = by_key[&body[64..64 * 7]];
        let p = &pools[id];
        let spacing = i32::try_from(signed(word(&p.key, 5) >> 16, 24)).expect("tickSpacing");
        let first = *p.words.keys().next().expect("слова");
        let last = *p.words.keys().last().expect("слова");
        let words: Vec<U256> = p.words.values().copied().collect();
        // Битмап записи разобран нашим кодом — те же тики, что прочитаны getPoolTickInfo.
        assert_eq!(
            set_ticks(first, &words, spacing),
            p.ticks.keys().copied().collect::<Vec<_>>()
        );
        let map = TickMap {
            spacing,
            ticks: Arc::new(p.ticks.clone()),
            words: (first, last),
        };
        let z = !word(body, 7).is_zero();
        let amount = word(body, 8);
        let remote = word(v["result"].as_str().expect("ответ"), 0);
        let buy = z == p.usdt0;
        let usdt_amt = if buy { (amount / usd).to::<u64>() } else { 0 };
        let lp = OVERRIDE
            .iter()
            .find(|(pre, a, _)| id.starts_with(pre) && buy && *a == usdt_amt)
            .map_or(p.lp, |o| o.2);
        let s = ClState {
            sqrt_price: p.sqrt,
            tick: p.tick,
            liquidity: p.liq,
            fee: swap_fee(p.pf, lp, z),
            ticks: &map,
            version: Version::V4,
        };
        let r = swap_cl::swap(&s, z, true, amount).expect("обмен");
        // CAKE 0xc528, покупка 20 и 100 USDT: цена уходит выше слова 28 — дальше слов, снятых серверным ИИ (±10).
        if id.starts_with("0xc528f2c0") && buy && usdt_amt >= 20 {
            assert!(!r.complete, "цена ушла за прочитанные слова записи");
            short += 1;
            continue;
        }
        assert!(r.complete, "{id} {amount}");
        assert_eq!(r.amount_out, remote, "{id} z={z} {amount}");
        if lp == p.lp { exact += 1 } else { over += 1 }
    }
    assert_eq!((exact, over, short), (12, 4, 2));
}
