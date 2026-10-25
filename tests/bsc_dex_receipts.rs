//! Файл: tests/bsc_dex_receipts.rs
//! Что делает: источник событий пулов «квитанции блоков по HTTP» (решение 68) на ЖИВОЙ записи узла BNB Chain
//! (tests/data/receipts_2026-10-25/responses.json: bsc-dataseed.bnbchain.org, 2026-10-25, записал техлид рекордером
//! tests/recorders/record_receipts.js по calls.json коммита abcd3f1; окно 129702236–129702239, блок создания пула
//! 129702218):
//! - логи квитанций фильтруются теми же фильтрами, что подписка WebSocket (pools/group.rs filter → node/log_filter.rs):
//!   подходят ровно логи наших пулов с событиями состояния (V2 Sync/Swap, V3 Swap/Burn …), чужие — нет;
//! - разобранные логи — тот же формат, что у подписки (alloy Log с blockNumber и logIndex) — разбор событий общий;
//! - цепочка блоков окна сходится по parentHash; чужой блок — реорг: отменённый блок снимается, его логи — removed: true;
//! - событие создания пула (PairCreated фабрики PancakeSwap V2) находится фильтром «новых пулов».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{node::{log_filter, receipts_chain}, pools::{group, events, families}}.

use std::collections::HashMap;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::node::log_filter::Filter;
use arb::venues::bsc_dex::node::receipts_chain::{Block, Chain, logs_of, route};
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::group::{Topics, filter};
use arb::venues::bsc_dex::pools::{events, families};

const DIR: &str = "tests/data/receipts_2026-10-25/";

fn load(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

/// Ответ записи по имени («header N», «receipts N …»).
fn result(rec: &Value, prefix: &str) -> Value {
    rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"].as_str().unwrap().starts_with(prefix))
        .unwrap()["response"]["result"]
        .clone()
}

/// Наши пулы V2/V3 из calls.json (вид — по DEX индексатора, как в node_calls_3).
fn our_pools() -> Vec<Found> {
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
fn filters(pools: &[Found]) -> Vec<Filter> {
    let mut by: HashMap<&str, Vec<&Found>> = HashMap::new();
    for f in pools {
        by.entry(f.family.id).or_default().push(f);
    }
    by.values()
        .map(|v| Filter::new(&filter(v, Topics::Full)))
        .collect()
}

#[test]
fn receipts_logs_route_like_ws_subscription() {
    let rec = load("responses.json");
    let pools = our_pools();
    let fs = filters(&pools);
    let refs: Vec<&Filter> = fs.iter().collect();
    let state: HashMap<Address, Vec<B256>> = pools
        .iter()
        .map(|f| match f.reference {
            PoolRef::Contract(a) => (a, events::topics(f.family)),
            PoolRef::Singleton { .. } => unreachable!(),
        })
        .collect();
    let mut kinds = Vec::new();
    for n in rec["window"].as_array().unwrap() {
        let n = n.as_u64().unwrap();
        let logs = logs_of(result(&rec, &format!("receipts {n}")).as_array().unwrap());
        let (_, any) = route(&refs, &logs);
        // Ожидание считается независимо: адрес — наш пул, topic0 — событие состояния его вида.
        let want: Vec<&Value> = logs
            .iter()
            .filter(|l| {
                let a: Address = l["address"].as_str().unwrap().parse().unwrap();
                let t0: Option<B256> = l["topics"][0].as_str().and_then(|s| s.parse().ok());
                state
                    .get(&a)
                    .is_some_and(|t| t0.is_some_and(|t0| t.contains(&t0)))
            })
            .collect();
        assert_eq!(any.len(), want.len(), "блок {n}");
        for l in &any {
            // Тот же формат, что у подписки: разбирается как alloy Log, есть блок и номер лога.
            let log: Log = serde_json::from_value(l.clone()).unwrap();
            assert_eq!(log.block_number, Some(n));
            assert!(log.log_index.is_some() && !log.removed);
            let a = log.address();
            let f = pools
                .iter()
                .find(|f| f.reference == PoolRef::Contract(a))
                .unwrap();
            kinds.push(f.family.kind);
        }
    }
    // В окне есть события и V2, и V3.
    assert!(
        kinds.iter().any(|k| format!("{k:?}").contains("V2")),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| format!("{k:?}").contains("V3")),
        "{kinds:?}"
    );
}

#[test]
fn chain_follows_parent_hash_and_rolls_back_on_reorg() {
    let rec = load("responses.json");
    let hdr = |n: u64| {
        let h = result(&rec, &format!("header {n}"));
        let p = |k: &str| h[k].as_str().unwrap().parse::<B256>().unwrap();
        (p("hash"), p("parentHash"))
    };
    let window: Vec<u64> = rec["window"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let mut chain = Chain::default();
    let start = window[0] - 1;
    chain.push(
        Block {
            number: start,
            hash: hdr(start).0,
            logs: vec![],
        },
        8,
    );
    for &n in &window {
        let (hash, parent) = hdr(n);
        assert!(chain.continues(parent), "блок {n} продолжает цепочку");
        let logs =
            vec![json!({ "address": "0x0", "topics": [], "blockNumber": format!("0x{n:x}") })];
        chain.push(
            Block {
                number: n,
                hash,
                logs,
            },
            8,
        );
    }
    // Блок не из этой цепочки (записанный 129702218) — его parentHash не совпадает: реорг.
    let other = rec["created"].as_u64().unwrap();
    assert!(!chain.continues(hdr(other).1));
    let (n, removed) = chain.pop_removed().unwrap();
    assert_eq!(n, *window.last().unwrap());
    assert!(removed.iter().all(|l| l["removed"] == json!(true)));
    assert_eq!(chain.top().unwrap().number, n - 1);
    // Окно ограничено глубиной.
    let mut short = Chain::default();
    for &x in &window {
        short.push(
            Block {
                number: x,
                hash: hdr(x).0,
                logs: vec![],
            },
            2,
        );
    }
    short.pop_removed();
    short.pop_removed();
    assert!(short.top().is_none());
}

#[test]
fn new_pool_event_found_by_created_filter() {
    let rec = load("responses.json");
    let calls = load("calls.json");
    let n = rec["created"].as_u64().unwrap();
    let logs = logs_of(result(&rec, &format!("receipts {n}")).as_array().unwrap());
    let pair_created = calls["created_topics"]["PairCreated"].clone();
    let f = Filter::new(&json!({ "topics": [[pair_created]] }));
    let found: Vec<&Value> = logs.iter().filter(|l| f.matches(l)).collect();
    assert!(!found.is_empty());
    // Фильтр по адресу фабрики сужает (адрес — из записанного события).
    let factory = found[0]["address"].clone();
    let g = Filter::new(
        &json!({ "address": factory, "topics": [[calls["created_topics"]["PairCreated"]]] }),
    );
    assert!(logs.iter().filter(|l| g.matches(l)).count() <= found.len());
}
