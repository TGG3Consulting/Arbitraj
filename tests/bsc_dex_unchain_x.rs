//! Проверка фактического пути SOL на BSC 23.11.2026. Вызывается cargo test.
//! Использует сырые ответы двух квотеров, состояния архивных блоков и настоящий Swap Unchain X.

mod route_common;

use std::collections::HashMap;

use alloy::rpc::types::Log;
use alloy_primitives::{Bytes, U256, address};
use arb::core::types::PoolState;
use arb::venues::bsc_dex::exec::executor::code::Version as Executor;
use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{events, exec_callback, exec_kind::ExecKind, families, verify};
use arb::venues::bsc_dex::quote::virtual_quoter;
use serde_json::Value;

const KIND: &str = "v3_fork_uniswap_lm";

/// Записанные публичные ответы узла. Битый файл — ошибка теста.
fn probe() -> Value {
    serde_json::from_str(include_str!("data/unchain_x_2026-11-23/bsc.json")).unwrap()
}

/// Разбирает запись пула теми же ABI slot0, liquidity и TickLens, что использует адаптер.
fn entry(v: &Value, unchain: bool) -> Entry {
    let word = v["word"].as_i64().unwrap() as i16;
    let raw = |key: &str| alloy_primitives::hex::decode(v[key].as_str().unwrap()).unwrap();
    let fields = HashMap::from([
        ("p slot0".to_owned(), raw("slot0")),
        ("p liquidity".to_owned(), raw("liquidity")),
        (format!("p TickLens word {word}"), raw("ticks")),
    ]);
    let eth = address!("2170ed0880ac9a755fd29b2688956bd959f933f8");
    let (coin, counter, fee) = if unchain {
        (
            address!("570a5d26f7765ecb712c0924e4de545b89fd43df"),
            eth,
            2500,
        )
    } else {
        (
            eth,
            address!("55d398326f99059ff775485246999027b3197955"),
            100,
        )
    };
    let mut e = route_common::v3(
        &fields,
        "p",
        !unchain,
        U256::from_be_slice(&raw("spacing")).to::<u32>() as i32,
        fee,
        (word, word),
        v["pool"].as_str().unwrap().parse().unwrap(),
        coin,
        counter,
    );
    if unchain {
        e.family = families::make("unchain-x", KIND).unwrap();
        e.pool.family = e.family.id.into();
    }
    e.pool.block = v["block"].as_u64().unwrap();
    e.synced_block = e.pool.block;
    e
}

/// Стоимость точного выхода, включая комиссию, по математике пула.
fn cost(e: &Entry, zero_for_one: bool, amount: U256) -> U256 {
    let PoolState::V3 {
        sqrt_price_x96,
        tick,
        liquidity,
    } = &e.pool.state
    else {
        panic!("не V3")
    };
    let s = ClState {
        sqrt_price: *sqrt_price_x96,
        tick: *tick,
        liquidity: *liquidity,
        fee: e.pool.fee_pips.unwrap(),
        ticks: e.ticks.as_ref().unwrap(),
        version: Version::V3,
    };
    let result = swap_cl::swap(&s, zero_for_one, false, amount).unwrap();
    assert!(result.complete);
    assert_eq!(result.amount_out, amount);
    result.amount_in
}

#[test]
fn uniswap_lm_passes_fields_registry_and_executor_callback() {
    let v = probe();
    let f = families::make("unchain-x", KIND).unwrap();
    let calls = &v["field_calls"];
    let bytes = |name: &str| calls[name].as_str().unwrap().parse::<Bytes>().unwrap();
    let fields = [
        "factory()",
        "token0()",
        "token1()",
        "fee()",
        "tickSpacing()",
        "lmPool()",
    ]
    .map(|name| Ok(bytes(name)));
    let mut read = verify::Read::default();
    verify::fill(f, &mut read, &fields).unwrap();
    assert_eq!(
        read.factory,
        address!("82fa7b2ce2a76c7888a9d3b0a81e0b2ecfd8d40c")
    );
    assert_eq!((read.fee, read.spacing), (Some(2500), Some(50)));
    let pool = address!("1cc4504a0d5254167520e2e36c35c82a932816fd");
    verify::registered(pool, &bytes("getPool(address,address,uint24)")).unwrap();
    assert!(
        verify::v3_abi(
            families::make("unchain-x", "v3_fork").unwrap(),
            fields.get(5)
        )
        .is_err()
    );
    assert!(verify::v3_abi(f, None).is_err());
    assert_eq!(f.executor_kind, Some(ExecKind::V3));
    assert!(f.graphable && f.fork && !f.tradable);
    assert!(virtual_quoter::supports(f.proto));
    // Те же код VirtualQuoter и callback, что при dex.quoter_virtual = да. Публичные eth_call
    // с подменой кода по адресу квотера на одном блоке, обе стороны, без ключа и транзакции.
    for q in v["virtual_quotes"].as_array().unwrap() {
        let amount = U256::from_str_radix(q["amount_in"].as_str().unwrap(), 10).unwrap();
        let raw: Bytes = q["quote"].as_str().unwrap().parse().unwrap();
        let official: Bytes = q["official"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            virtual_quoter::decode(&raw, amount),
            Some(Ok(U256::from_be_slice(&official[..32])))
        );
    }
    for version in [Executor::V2, Executor::V3, Executor::V4, Executor::V5] {
        assert_eq!(
            exec_callback::check(version, "uniswapV3SwapCallback(int256,int256,bytes)"),
            None
        );
    }
}

#[test]
fn recorded_swap_brings_plan_to_the_verified_price() {
    let v = probe();
    let states = &v["states"];
    let mut un = entry(&states["un_old"], true);
    let pcs = entry(&states["pc_fresh"], false);
    let sol = U256::from(2_063_000_000_000_000_000u64);
    // output → input: сколько ETH за SOL, затем сколько USDT за этот ETH.
    let original_eth = cost(&un, true, sol);
    let original_usdt = cost(&pcs, false, original_eth);
    assert_eq!(original_usdt, U256::from(156_446_247_964_454_233_687u128));
    let log: Log = serde_json::from_value(v["last_swap"].clone()).unwrap();
    let old_family = families::make("unchain-x", "v3_fork_pancake").unwrap();
    let topic = log.topic0().unwrap();
    assert!(!events::topics(old_family).contains(topic));
    assert!(events::topics(un.family).contains(topic));
    assert_eq!(events::apply(&mut un, &log), events::Applied::Updated);
    let settled = entry(&states["un_fresh"], true);
    assert_eq!(un.pool.state, settled.pool.state);
    let fresh_eth = cost(&un, true, sol);
    let fresh_usdt = cost(&pcs, false, fresh_eth);
    assert_eq!(fresh_usdt, U256::from(158_311_599_774_733_016_410u128));
    let pcs_next = entry(&states["pc_control"], false);
    // По каждому входу сверяем все 256 бит суммы с архивным ответом официального QuoterV2.
    let local = [
        original_eth,
        fresh_eth,
        cost(&pcs_next, false, original_eth),
        cost(&pcs_next, false, fresh_eth),
        original_usdt,
        fresh_usdt,
    ];
    let official = v["quotes"].as_array().unwrap();
    assert_eq!(official.len(), local.len());
    for (got, call) in local.into_iter().zip(official) {
        let result: Bytes = call["result"].as_str().unwrap().parse().unwrap();
        assert_eq!(got, U256::from_be_slice(&result[..32]));
    }
}
