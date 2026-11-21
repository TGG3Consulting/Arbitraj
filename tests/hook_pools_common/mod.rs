//! Файл: tests/hook_pools_common/mod.rs
//! Что делает: чтение записи узла tests/data/hook_pools_2026-11-16/rpc.json.zst (37 пулов PancakeSwap Infinity CL с
//! крючком, блоки 133993052…133993054) — ключ пула (poolIdToPoolKey), slot0 (getSlot0), ликвидность (getLiquidity),
//! ответы CLQuoter.quoteExactInputSingle (три from, обе стороны, 20 и 1 USDT) и последние события Swap пула; расчёт
//! «нашей» математикой одного шага (math/swap_step.rs step_v4 — тот же шаг, что зовёт math/swap_cl.rs) с комиссией
//! quote/pool_quote.rs swap_fee.
//! Кто вызывает: tests/bsc_dex_hook_pools_1116.rs, tests/bsc_dex_hook_pools_1116_rules.rs,
//! tests/bsc_dex_hook_pools_1116_b1.rs (каждому нужна своя часть — allow(dead_code)).
//! От чего зависит: arb::venues::bsc_dex::math::{swap_step, tick_math}; serde_json, zstd.
//!
//! Раскладка ответов — ABI контрактов (infinity-core 3c5ef1be src/pool-cl/CLPoolManager.sol getSlot0 / getLiquidity /
//! poolIdToPoolKey; infinity-periphery 56a7a4f5 src/pool-cl/lens/CLQuoter.sol quoteExactInputSingle — (amountOut,
//! gasEstimate); событие Swap — ICLPoolManager.sol: amount0, amount1, sqrtPriceX96, liquidity, tick, fee, protocolFee).
#![allow(dead_code)]

use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, I256, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_step::step_v4;
use arb::venues::bsc_dex::math::tick_math::{max_sqrt_ratio, min_sqrt_ratio};

pub const DIR: &str = "tests/data/hook_pools_2026-11-16";
/// Селекторы вызовов записи (первые 4 байта calldata, как в записи).
const KEY: &str = "0x0e2d484a";
const SLOT0: &str = "0xc815641c";
const LIQ: &str = "0xfa6793d5";
const QUOTE: &str = "0x9938b8ed";
/// topic0 события Swap CLPoolManager (из записи eth_getLogs).
const SWAP: &str = "0x04206ad2b7c0f463bff3dd4f33c5735b0f2957a351e4f79763a4fa9e775dd237";

/// Состояние пула на блоке.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub sqrt: U256,
    pub protocol_fee: u32,
    pub lp_fee: u32,
    pub liquidity: u128,
}

/// Пул записи.
#[derive(Debug, Clone, Default)]
pub struct Pool {
    /// Слова ключа (currency0, currency1, hooks, manager, fee, parameters) — hex без 0x, как в calldata квотера.
    pub key_words: String,
    pub currency0: Address,
    pub currency1: Address,
    pub hooks: Address,
    pub manager: Address,
    pub fee: u32,
    pub parameters: B256,
    pub at: BTreeMap<u64, State>,
    /// Последние события Swap: (fee, protocolFee).
    pub swaps: Vec<(u32, u32)>,
}

/// Вызов квотера.
#[derive(Debug, Clone)]
pub struct Quote {
    pub pool: String,
    pub block: u64,
    pub from: Option<String>,
    pub zero_for_one: bool,
    pub amount: U256,
    /// Ok(amountOut) или Err(данные отката, hex).
    pub out: Result<U256, String>,
    /// Текст отказа узла (error.message) как есть; None — ответ без ошибки.
    pub message: Option<String>,
}

pub struct Rec {
    pub pools: BTreeMap<String, Pool>,
    pub quotes: Vec<Quote>,
}

/// i-е 32-байтовое слово hex-строки (с 0x или без).
pub fn word(hex: &str, i: usize) -> U256 {
    let h = hex.strip_prefix("0x").unwrap_or(hex);
    U256::from_str_radix(&h[64 * i..64 * i + 64], 16).expect("слово hex")
}

fn small(hex: &str, i: usize) -> u32 {
    word(hex, i).to::<u32>()
}

fn addr(hex: &str, i: usize) -> Address {
    Address::from_word(B256::from(word(hex, i).to_be_bytes::<32>()))
}

fn block(v: &Value) -> u64 {
    let b = v["params"][1].as_str().expect("блок");
    u64::from_str_radix(b.trim_start_matches("0x"), 16).expect("блок hex")
}

/// Запись целиком. Паника — файла нет или он не по формату записи.
pub fn load() -> Rec {
    let f = std::fs::File::open(format!("{DIR}/rpc.json.zst")).expect("запись");
    let all: Vec<Value> =
        serde_json::from_reader(zstd::Decoder::new(f).expect("zstd")).expect("json записи");
    let mut pools: BTreeMap<String, Pool> = BTreeMap::new();
    let mut raw_quotes = Vec::new();
    for v in &all {
        match v["method"].as_str() {
            Some("eth_call") => {}
            Some("eth_getLogs") => {
                let id = v["params"][0]["topics"][1]
                    .as_str()
                    .expect("PoolId")
                    .to_lowercase();
                let p = pools.entry(id).or_default();
                for l in v["result"].as_array().expect("логи") {
                    if l["topics"][0].as_str() == Some(SWAP) {
                        let d = l["data"].as_str().expect("data");
                        p.swaps.push((small(d, 5), small(d, 6)));
                    }
                }
                continue;
            }
            _ => continue,
        }
        let data = v["params"][0]["data"].as_str().expect("calldata");
        let sel = &data[..10];
        if sel == QUOTE {
            raw_quotes.push(v);
            continue;
        }
        let id = format!("0x{}", &data[10..74]).to_lowercase();
        let res = v["result"].as_str().expect("ответ");
        let p = pools.entry(id).or_default();
        let b = block(v);
        match sel {
            KEY => {
                p.key_words = res[2..2 + 64 * 6].to_lowercase();
                p.currency0 = addr(res, 0);
                p.currency1 = addr(res, 1);
                p.hooks = addr(res, 2);
                p.manager = addr(res, 3);
                p.fee = small(res, 4);
                p.parameters = B256::from(word(res, 5).to_be_bytes::<32>());
            }
            SLOT0 => {
                let s = p.at.entry(b).or_default();
                s.sqrt = word(res, 0);
                s.protocol_fee = small(res, 2);
                s.lp_fee = small(res, 3);
            }
            LIQ => {
                let s = p.at.entry(b).or_default();
                s.liquidity = word(res, 0).to::<u128>();
            }
            _ => {}
        }
    }
    let by_key: BTreeMap<String, String> = pools
        .iter()
        .map(|(id, p)| (p.key_words.clone(), id.clone()))
        .collect();
    let quotes = raw_quotes
        .into_iter()
        .map(|v| {
            let call = &v["params"][0];
            let body = &call["data"].as_str().expect("calldata")[10..];
            let pool = by_key
                .get(&body[64..64 * 7].to_lowercase())
                .expect("ключ квотера есть среди пулов")
                .clone();
            let out = match v["result"].as_str() {
                Some(r) => Ok(word(r, 0)),
                None => Err(v["error"]["data"].as_str().expect("откат").to_string()),
            };
            Quote {
                pool,
                block: block(v),
                from: call["from"].as_str().map(str::to_lowercase),
                zero_for_one: !word(body, 7).is_zero(),
                amount: word(body, 8),
                out,
                message: v["error"]["message"].as_str().map(str::to_string),
            }
        })
        .collect();
    Rec { pools, quotes }
}

/// Наш выход одним шагом SwapMath V4 до предела цены квотера (MIN_SQRT_RATIO + 1 / MAX_SQRT_RATIO − 1): (выход, вся
/// сумма вошла в шаг). Комиссия — в пипсах. Паника — шаг откатился бы.
pub fn one_step(s: &State, zero_for_one: bool, amount: U256, fee: u32) -> (U256, bool) {
    let target = if zero_for_one {
        min_sqrt_ratio() + U256::from(1u8)
    } else {
        max_sqrt_ratio() - U256::from(1u8)
    };
    let signed = I256::try_from(amount).expect("сумма");
    let (next, a_in, out, fee_amount) =
        step_v4(s.sqrt, target, s.liquidity, -signed, fee).expect("шаг V4");
    (out, next != target && a_in + fee_amount == amount)
}

/// USDT записи — токен, который чаще всех встречается в ключах пулов записи (35 из 37 пар — с USDT, README записи).
/// Паника — пулов нет.
pub fn usdt(rec: &Rec) -> Address {
    let mut n: BTreeMap<Address, usize> = BTreeMap::new();
    for p in rec.pools.values() {
        *n.entry(p.currency0).or_insert(0) += 1;
        *n.entry(p.currency1).or_insert(0) += 1;
    }
    n.into_iter().max_by_key(|(_, c)| *c).expect("токены").0
}

/// a − b со знаком (i128). Паника — разница не помещается в i128.
pub fn signed(a: U256, b: U256) -> i128 {
    if a >= b {
        i128::try_from(a - b).expect("разница в i128")
    } else {
        -i128::try_from(b - a).expect("разница в i128")
    }
}

/// Короткое имя пула для сообщений: первые 10 знаков PoolId.
pub fn short(id: &str) -> &str {
    &id[..10]
}
