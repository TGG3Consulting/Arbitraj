//! Файл: tests/bsc_dex_hook_pools_batch_1117.rs
//! Что проверяет: решение «335 (В1, пакет)» на досъёмке узла 17.11 (tests/data/hook_pools_ticks_2026-11-17, блок
//! 134000293): пакет Multicall3.aggregate3 = отдельные eth_call. Наш node/multicall.rs encode из записанных отдельных
//! вызовов (RIVER, CAKE 0xc610, BLESS: getSlot0, getLiquidity, квотер на 20 USDT в обе стороны) даёт ту же calldata,
//! что записанный aggregate3 (allowFailure = true); decode его ответа — те же байты getSlot0 / getLiquidity и тот же
//! amountOut квотера (второе слово — gasEstimate — у пакета другое); откат BLESS в пакете и отдельным вызовом
//! разбирается check/hook_batch_parse.rs classify одинаково — NotEnoughLiquidity(PoolId).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{node::multicall, check::hook_batch_parse}.

use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::check::hook_batch_parse::{Fail, classify};
use arb::venues::bsc_dex::node::multicall;

const DIR: &str = "tests/data/hook_pools_ticks_2026-11-17";
const H3: u64 = 134_000_293;
const QUOTE: &str = "0x9938b8ed";
const AGG: &str = "0x82ad56cb";

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

#[test]
fn batch_equals_single_calls() {
    let all = load();
    let at: Vec<&Value> = all
        .iter()
        .filter(|v| v["method"] == "eth_call" && block(v) == H3)
        .collect();
    let k = at
        .iter()
        .position(|v| data(v).starts_with(AGG))
        .expect("aggregate3");
    let single = &at[k + 1..];
    let calls: Vec<(Address, Bytes)> = single
        .iter()
        .map(|v| {
            (
                Address::from_str(v["params"][0]["to"].as_str().expect("to")).expect("адрес"),
                hex(data(v)),
            )
        })
        .collect();
    assert_eq!(calls.len(), 12);
    assert_eq!(
        multicall::encode(&calls),
        hex(data(at[k])),
        "наш пакет = записанный"
    );
    let got = multicall::decode(&hex(at[k]["result"].as_str().expect("ответ")), calls.len())
        .expect("aggregate3");
    let mut shorts = 0;
    for ((v, g), (_, cd)) in single.iter().zip(&got).zip(&calls) {
        let quote = cd.starts_with(&hex(QUOTE));
        match (v["result"].as_str(), g) {
            (Some(r), Ok(b)) if quote => assert_eq!(word(r, 0), U256::from_be_slice(&b[..32])),
            (Some(r), Ok(b)) => assert_eq!(hex(r), *b),
            (None, Err(text)) => {
                let alone = v["error"]["message"].as_str().expect("откат");
                assert_eq!(classify(text), classify(alone));
                assert!(matches!(classify(text), Fail::Short(_)), "{text}");
                shorts += 1;
            }
            other => panic!("пакет и отдельный вызов разошлись: {other:?}"),
        }
    }
    assert_eq!(shorts, 2, "BLESS — обе стороны");
}
