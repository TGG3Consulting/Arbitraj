//! Файл: tests/bsc_dex_receipts_catchup.rs
//! Что делает: догон ленты квитанций без сброса (решение 72):
//! - отметка «данные пулов отстают на N блоков» запрещает котировку и сделку (StaleData), снятая — разрешает;
//! - пропуск невосполним, только если ВСЕ узлы ответили «блока нет» (null в живой batch-записи блока 129761640 —
//!   заголовок убран): тогда Missing, иначе — повтор (один узел отстаёт, другой упал или ответил неполно);
//! - блоки конвейера распределяются по узлам списка по кругу, у каждого узла — свой темп.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common (окружение на настоящей SQLite), arb::venues::bsc_dex::node::{events_lag,
//! receipts_fetch, receipts_nodes, rpc_race, rpc_capped}.

mod native_common;

use std::time::Duration;

use serde_json::Value;

use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::node::events_lag::EventsLag;
use arb::venues::bsc_dex::node::receipts_fetch::{MISSING, nowhere, pair};
use arb::venues::bsc_dex::node::receipts_nodes::{Nodes, pick};
use arb::venues::bsc_dex::node::rpc_capped::Capped;
use arb::venues::bsc_dex::node::rpc_race::first_accepted;

fn batch_block() -> (u64, Value) {
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/receipts_batch_2026-10-25/responses.json").unwrap(),
    )
    .unwrap();
    let r = &rec["results"][1];
    let n = u64::from_str_radix(
        r["request"][0]["params"][0]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    (n, r["response"].clone())
}

#[tokio::test]
async fn lag_mark_forbids_quotes_until_caught_up() {
    let ctx = native_common::ctx("catchup-lag").await;
    let lag = EventsLag::default();
    assert!(lag.check(&ctx).is_ok());
    lag.set(52);
    let e = lag.check(&ctx).unwrap_err();
    assert_eq!(e.code, ErrorCode::StaleData);
    assert!(e.message.contains("отстают на 52 блоков"), "{}", e.message);
    lag.set(0);
    assert!(lag.check(&ctx).is_ok(), "догнали — котировки снова есть");
}

#[tokio::test]
async fn gap_is_unrecoverable_only_if_every_node_has_no_block() {
    let (n, full) = batch_block();
    let mut null = full.clone();
    null[0]["result"] = Value::Null;
    let mut short = full.clone();
    short[1]["result"].as_array_mut().unwrap().pop();
    let accept = |list| pair(n, list).map(|_| ());
    let answer = |v: Value| async move { Ok(Capped::Got(v.to_string())) };
    // Оба узла: блока нет — Missing (пропуск невосполним, если блок позади головы).
    let l = first_accepted(vec![answer(null.clone()), answer(null.clone())], 2, accept)
        .await
        .unwrap_err();
    assert!(l.rejected.iter().all(|r| r.starts_with(MISSING)));
    assert!(nowhere(&l), "Missing — пропуск невосполним");
    // Один узел: блока нет, другой ответил неполно (у него блок есть) — повтор, не сброс.
    let accept2 = |list| {
        let (h, r) = pair(n, list)?;
        arb::venues::bsc_dex::node::receipts_chain::complete(n, h.hash, &h.transactions, &r)
    };
    let l = first_accepted(vec![answer(null), answer(short)], 2, accept2)
        .await
        .unwrap_err();
    assert!(!nowhere(&l), "повтор, не сброс");
}

#[tokio::test]
async fn blocks_spread_over_nodes_each_with_own_pace() {
    let urls: Vec<String> = [
        "https://bsc-dataseed1.bnbchain.org",
        "https://bsc-dataseed2.bnbchain.org",
    ]
    .map(String::from)
    .to_vec();
    // Соседние блоки — на разные узлы.
    assert_ne!(pick(&urls, 129761640), pick(&urls, 129761641));
    assert_eq!(pick(&urls, 129761640), pick(&urls, 129761642));
    // Темп каждого узла свой (без часов: моменты считаются от одного «сейчас»): batch — вес 2, 20 вызовов/с → 0,1 с.
    let nodes = Nodes::default();
    let now = tokio::time::Instant::now();
    assert_eq!(
        nodes.reserve(&urls[0], 2, 20, now),
        now,
        "первый вызов — сразу"
    );
    assert_eq!(
        nodes.reserve(&urls[1], 2, 20, now),
        now,
        "другой узел не ждёт первого"
    );
    assert_eq!(
        nodes.reserve(&urls[0], 2, 20, now),
        now + Duration::from_millis(100),
        "тот же узел ждёт своего темпа"
    );
}
