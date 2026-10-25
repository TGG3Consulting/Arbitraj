//! Файл: tests/bsc_dex_receipts_chain.rs
//! Что делает: лента квитанций (решение 68) на ЖИВЫХ записях узла BNB Chain (tests/data/receipts_2026-10-25):
//! - цепочка блоков окна сходится по parentHash; чужой хеш — реорг (только подтверждённый вторым ответом): отменённые
//!   блоки снимаются, их настоящие логи наших пулов — removed: true;
//! - квитанции полные (по хешам транзакций заголовка) и этого блока, иначе — повтор с причиной;
//! - Infinity CL и Uniswap V4 — на записи responses_3.json (окно 129703969–129703972, PoolId из seen): фильтры по
//!   PoolId у менеджера отбирают ровно их события состояния.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/receipts_common, arb::venues::bsc_dex::{node::receipts_chain, pools::{events, families}}.

mod receipts_common;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256};

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::node::log_filter::Filter;
use arb::venues::bsc_dex::node::receipts_chain::{Block, Chain, complete, gone, logs_of, route};
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::{events, families};
use receipts_common::{filters, hdr, load, our_pools, ours, result};

#[test]
fn chain_follows_parent_hash_and_rolls_back_on_reorg() {
    let rec = load("responses.json");
    let pools = our_pools();
    let fs = filters(&pools);
    let refs: Vec<&Filter> = fs.iter().collect();
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
            hash: hdr(&rec, start).0,
            logs: vec![],
        },
        8,
    );
    for &n in &window {
        let (hash, parent, _) = hdr(&rec, n);
        assert!(chain.continues(parent), "блок {n} продолжает цепочку");
        chain.push(
            Block {
                number: n,
                hash,
                logs: ours(&rec, n, &refs),
            },
            8,
        );
    }
    // Узел отдаёт для блока 129702237 (в нём логи V2 и V3 наших пулов) другой хеш — записанный хеш блока создания.
    let other = hdr(&rec, rec["created"].as_u64().unwrap()).0;
    assert!(!chain.continues(other), "чужой родитель — реорг");
    // Ложный реорг за балансировщиком: второй ответ — прежний хеш, блок не снимается.
    let top = chain.top().unwrap().clone();
    assert!(!gone(top.hash, other, top.hash));
    assert!(gone(top.hash, other, other));
    // Снимаем до блока 129702237: его настоящие логи наших пулов уходят с removed: true.
    let target = window[1];
    let mut removed = Vec::new();
    while chain.top().unwrap().number >= target {
        removed.extend(chain.pop_removed().unwrap().1);
    }
    let want = ours(&rec, window[1], &refs).len()
        + ours(&rec, window[2], &refs).len()
        + ours(&rec, window[3], &refs).len();
    assert_eq!(removed.len(), want);
    assert!(want > 0);
    for l in &removed {
        let log: Log = serde_json::from_value(l.clone()).unwrap();
        assert!(log.removed, "отменённый лог — removed: true");
    }
    assert_eq!(chain.top().unwrap().number, window[0]);
    // Окно ограничено глубиной: за его пределами откатить нечего.
    let mut short = Chain::default();
    for &x in &window {
        short.push(
            Block {
                number: x,
                hash: hdr(&rec, x).0,
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
fn receipts_must_be_complete_and_of_this_block() {
    let rec = load("responses.json");
    for n in rec["window"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
    {
        let (hash, _, txs) = hdr(&rec, n);
        let r = result(&rec, &format!("receipts {n}"));
        let r = r.as_array().unwrap();
        assert_eq!(complete(n, hash, &txs, r), Ok(()), "блок {n}");
        // Узел отдал не все квитанции — повтор с причиной.
        let short = &r[..r.len() - 1];
        let why = complete(n, hash, &txs, short).unwrap_err();
        assert!(why.contains("неполные квитанции блока"), "{why}");
        // Пустой массив при непустом блоке — тоже неполные.
        assert!(complete(n, hash, &txs, &[]).is_err());
        // Квитанции другого блока (хеш другой) — реорг идёт.
        let other = hdr(&rec, rec["created"].as_u64().unwrap()).0;
        assert!(
            complete(n, other, &txs, r)
                .unwrap_err()
                .contains("другого хеша")
        );
    }
}

/// Infinity CL и Uniswap V4 на живой записи responses_3.json: PoolId, встретившиеся у менеджеров в окне (seen),
/// берутся как «наши» — фильтры подписки (PoolId в topics[1] у менеджера) отбирают ровно их события состояния.
#[test]
fn cl_and_v4_logs_route_by_pool_id() {
    let rec = load("responses_3.json");
    let cl: Address = "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b"
        .parse()
        .unwrap();
    let mut pools = Vec::new();
    for s in rec["seen"].as_array().unwrap() {
        let manager: Address = s["manager"].as_str().unwrap().parse().unwrap();
        let id: B256 = s["id"].as_str().unwrap().parse().unwrap();
        let family = if manager == cl {
            families::make("pancakeswap-infinity-clmm", "pancakeswap_infinity_cl").unwrap()
        } else {
            families::make("uniswap-v4-bsc", "uniswap_v4").unwrap()
        };
        let reference = PoolRef::Singleton { manager, id };
        if pools.iter().any(|f: &Found| f.reference == reference) {
            continue;
        }
        pools.push(Found {
            family,
            reference,
            token0: Address::ZERO,
            token1: Address::ZERO,
            coin: Address::ZERO,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            key: None,
            hooks: None,
        });
    }
    let fs = filters(&pools);
    let refs: Vec<&Filter> = fs.iter().collect();
    let (mut cl_n, mut v4_n) = (0, 0);
    for n in rec["window"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
    {
        let logs = logs_of(result(&rec, &format!("receipts {n}")).as_array().unwrap());
        let (_, any) = route(&refs, &logs);
        // Независимо: менеджер и PoolId — из наших, topic0 — событие состояния вида.
        let want = logs
            .iter()
            .filter(|l| {
                let a: Address = l["address"].as_str().unwrap().parse().unwrap();
                let id: Option<B256> = l["topics"][1].as_str().and_then(|s| s.parse().ok());
                let t0: Option<B256> = l["topics"][0].as_str().and_then(|s| s.parse().ok());
                pools.iter().any(|f| {
                    id.is_some_and(|id| f.reference == PoolRef::Singleton { manager: a, id })
                        && t0.is_some_and(|t| events::topics(f.family).contains(&t))
                })
            })
            .count();
        assert_eq!(any.len(), want, "блок {n}");
        for l in &any {
            let log: Log = serde_json::from_value(l.clone()).unwrap();
            assert_eq!(log.block_number, Some(n));
            if log.address() == cl {
                cl_n += 1;
            } else {
                v4_n += 1;
            }
        }
    }
    assert!(cl_n > 0 && v4_n > 0, "CL {cl_n}, V4 {v4_n}");
}
