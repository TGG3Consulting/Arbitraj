//! Файл: tests/receipts_common/mod.rs
//! Что делает: общее для тестов ленты квитанций (решение 68): чтение записей tests/data/receipts_2026-10-25, наши пулы
//! V2/V3 из calls.json, фильтры подписок как у pools/group.rs, заголовки и логи наших пулов из записи.
//! Кто вызывает: tests/bsc_dex_receipts.rs, tests/bsc_dex_receipts_chain.rs, tests/bsc_dex_receipts_pipe.rs.
//! От чего зависит: arb::venues::bsc_dex::{node::{log_filter, receipts_chain}, pools::{group, families, discover}}.
#![allow(dead_code)]

use std::collections::HashMap;

use alloy_primitives::{Address, B256};
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::node::log_filter::Filter;
use arb::venues::bsc_dex::node::receipts_chain::{logs_of, route};
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::group::{Topics, filter};

pub const DIR: &str = "tests/data/receipts_2026-10-25/";

pub fn load(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

/// Ответ записи по имени («header N», «receipts N …»).
pub fn result(rec: &Value, prefix: &str) -> Value {
    rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"].as_str().unwrap().starts_with(prefix))
        .unwrap()["response"]["result"]
        .clone()
}

/// Наши пулы V2/V3 из calls.json (вид — по DEX индексатора, как в node_calls_3).
pub fn our_pools() -> Vec<Found> {
    let calls = load("calls.json");
    calls["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let dex = p["tag"].as_str().unwrap().split(' ').nth(1).unwrap();
            let kind = match dex {
                "pancakeswap_v2" => "pancakeswap_v2",
                "pancakeswap-v3-bsc" => "pancakeswap_v3",
                "uniswap-bsc" => "uniswap_v3",
                other => panic!("{other}"),
            };
            let a: Address = p["addr"].as_str().unwrap().parse().unwrap();
            Found {
                family: families::make(dex, kind).unwrap(),
                reference: PoolRef::Contract(a),
                token0: Address::ZERO,
                token1: Address::ZERO,
                coin: Address::ZERO,
                fee_pips: None,
                tick_spacing: None,
                bin_step: None,
                key: None,
                hooks: None,
            }
        })
        .collect()
}

/// Фильтры подписок «все события» — как pools/group.rs: одна подписка на вид пулов.
pub fn filters(pools: &[Found]) -> Vec<Filter> {
    let mut by: HashMap<&str, Vec<&Found>> = HashMap::new();
    for f in pools {
        by.entry(f.family.id).or_default().push(f);
    }
    by.values()
        .map(|v| Filter::new(&filter(v, Topics::Full)))
        .collect()
}

/// Заголовок записи: (хеш, родитель, хеши транзакций).
pub fn hdr(rec: &Value, n: u64) -> (B256, B256, Vec<B256>) {
    let h = result(rec, &format!("header {n}"));
    let p = |k: &str| h[k].as_str().unwrap().parse::<B256>().unwrap();
    let txs = h["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().parse().unwrap())
        .collect();
    (p("hash"), p("parentHash"), txs)
}

/// Логи наших пулов блока окна — как их раздаёт лента.
pub fn ours(rec: &Value, n: u64, fs: &[&Filter]) -> Vec<Value> {
    let logs = logs_of(result(rec, &format!("receipts {n}")).as_array().unwrap());
    route(fs, &logs).1
}
