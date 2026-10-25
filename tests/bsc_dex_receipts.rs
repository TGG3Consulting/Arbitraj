//! Файл: tests/bsc_dex_receipts.rs
//! Что делает: источник событий пулов «квитанции блоков по HTTP» (решение 68) на ЖИВОЙ записи узла BNB Chain
//! (tests/data/receipts_2026-10-25/responses.json: bsc-dataseed.bnbchain.org, 2026-10-25, записал техлид рекордером
//! tests/recorders/record_receipts.js по calls.json коммита abcd3f1; окно 129702236–129702239, блок создания пула
//! 129702218):
//! - логи квитанций фильтруются теми же фильтрами, что подписка WebSocket (pools/group.rs filter → node/log_filter.rs):
//!   подходят ровно логи наших пулов с событиями состояния (V2 Sync/Swap, V3 Swap/Burn …), чужие — нет;
//! - разобранные логи — тот же формат, что у подписки (alloy Log с blockNumber и logIndex) — разбор событий общий;
//! - логи блока и номер блока раздаются одним действием (подписка после раздачи видит номер блока), обрыв ленты
//!   закрывает подписки;
//! - событие создания пула (PairCreated фабрики PancakeSwap V2) находится фильтром «новых пулов».
//!
//! Кто вызывает: cargo test.
//! Цепочка, реорг, полнота квитанций, Infinity CL и Uniswap V4 — tests/bsc_dex_receipts_chain.rs.
//! От чего зависит: tests/receipts_common, arb::venues::bsc_dex::{node::{receipts, receipts_chain}, pools::{group, events}}.

mod receipts_common;

use std::collections::HashMap;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::node::log_filter::Filter;
use arb::venues::bsc_dex::node::receipts::Feed;
use arb::venues::bsc_dex::node::receipts_chain::{logs_of, route};
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::events;
use arb::venues::bsc_dex::pools::group::{Topics, filter};
use receipts_common::{filters, load, our_pools, result};

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

/// Замечание ревью #58: логи блока и номер блока — одним действием. Пачка, подписавшаяся после раздачи блока N,
/// видит eth_blockNumber = N и читает состояние на N; подписавшаяся до — получает логи N.
#[tokio::test]
async fn deliver_sets_block_number_together_with_logs() {
    let rec = load("responses.json");
    let pools = our_pools();
    let fs = filters(&pools);
    let feed = Feed::default();
    let before = feed.open_idle();
    let mut by_kind: HashMap<&str, Vec<&Found>> = HashMap::new();
    for p in &pools {
        by_kind.entry(p.family.id).or_default().push(p);
    }
    let mut subs = Vec::new();
    for v in by_kind.values() {
        subs.push(
            before
                .subscribe(&json!(["logs", filter(v, Topics::Full)]))
                .unwrap(),
        );
    }
    let n = rec["window"][1].as_u64().unwrap();
    let logs = logs_of(result(&rec, &format!("receipts {n}")).as_array().unwrap());
    let refs: Vec<&Filter> = fs.iter().collect();
    let want = route(&refs, &logs).1.len();
    let any = feed.deliver(&logs, n);
    assert_eq!(any.len(), want);
    let mut got = 0;
    for s in &mut subs {
        while let Ok(raw) = s.rx.try_recv() {
            let log: Log = serde_json::from_str(raw.get()).unwrap();
            assert_eq!(log.block_number, Some(n));
            got += 1;
        }
    }
    assert_eq!(got, want);
    // Подписка после раздачи: номер уже N — логи N учтены в чтении на N, не потеряны.
    let after = feed.open_idle();
    let _sub = after.subscribe(&json!(["logs", {"address": []}])).unwrap();
    let b = after.request("eth_blockNumber", &json!([])).await.unwrap();
    assert_eq!(b, json!(format!("0x{n:x}")));
    // Обрыв ленты: подписки закрыты, «соединение» знает причину.
    feed.cut("проверка");
    assert!(after.end_reason().unwrap().contains("проверка"));
    assert!(after.subscribe(&json!(["logs", {}])).is_err());
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
