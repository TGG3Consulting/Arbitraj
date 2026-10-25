//! Файл: tests/bsc_dex_receipts_catchup.rs
//! Что делает: догон ленты квитанций без сброса (решение 72):
//! - отметка «данные пулов отстают на N блоков» запрещает котировку и сделку (StaleData), снятая — разрешает;
//! - пропуск невосполним, только если блок не отдал ни один узел, у которого своя голова не ниже блока (null в живой
//!   batch-записи блока 129761640 — заголовок убран); узлы ещё не дошли или один ответил неполно — повтор;
//! - снятие отметки будит циклы проверок;
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
use arb::venues::bsc_dex::node::rpc_race::{Reject, first_accepted};

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

/// Ревью #62: публичные узлы за балансировщиком отстают от основного на 1–3 блока — null на блок n ещё не «невосполнимо».
/// Невосполнимо, только если блок не отдал ни один узел, у которого СВОЯ голова (eth_blockNumber в той же пачке) не
/// ниже n. Ответы — живая batch-запись блока 129761640; null — из неё же (заголовок убран), головы узлов — как ответ
/// eth_blockNumber.
#[tokio::test]
async fn gap_is_unrecoverable_only_if_nodes_reached_the_block() {
    let (n, full) = batch_block();
    let with_head = |mut v: Value, head: u64| {
        v.as_array_mut().unwrap().push(
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "result": format!("0x{head:x}") }),
        );
        v
    };
    let mut null = full.clone();
    null[0]["result"] = Value::Null;
    let mut short = full.clone();
    short[1]["result"].as_array_mut().unwrap().pop();
    let accept = |list: Vec<_>| {
        let head = list
            .get(2)
            .and_then(
                |r: &arb::gateway::errors::GatewayResult<Box<serde_json::value::RawValue>>| {
                    r.as_ref().ok()
                },
            )
            .and_then(|r| serde_json::from_str::<alloy_primitives::U64>(r.get()).ok())
            .map(|h| h.to::<u64>());
        let reject = |why: String| Reject { why, head };
        let (h, r) = pair(n, list).map_err(reject)?;
        arb::venues::bsc_dex::node::receipts_chain::complete(n, h.hash, &h.transactions, &r)
            .map_err(reject)?;
        Ok(())
    };
    let answer = |v: Value| async move { Ok(Capped::Got(v.to_string())) };
    // Все null, головы узлов ниже n — узлы ещё не дошли: повтор.
    let runs = vec![
        answer(with_head(null.clone(), n - 1)),
        answer(with_head(null.clone(), n - 3)),
    ];
    let l = first_accepted(runs, 3, accept).await.unwrap_err();
    assert!(l.rejected.iter().all(|r| r.why.starts_with(MISSING)));
    assert!(!nowhere(&l, n), "узлы не дошли до блока — повтор, не сброс");
    // Головы узлов не ниже n, а блок null у всех — невосполнимо.
    let runs = vec![
        answer(with_head(null.clone(), n)),
        answer(with_head(null.clone(), n + 2)),
    ];
    let l = first_accepted(runs, 3, accept).await.unwrap_err();
    assert!(nowhere(&l, n), "дошли и не отдают — пропуск невосполним");
    // Один дошёл и null, другой ответил неполно — повтор.
    let runs = vec![
        answer(with_head(null, n + 1)),
        answer(with_head(short, n + 1)),
    ];
    let l = first_accepted(runs, 3, accept).await.unwrap_err();
    assert!(!nowhere(&l, n));
    // Полный ответ с головой — принят.
    let ok = first_accepted(vec![answer(with_head(full, n))], 3, accept).await;
    assert!(ok.is_ok());
}

/// Снятие отметки «отстают» будит циклы проверок (они вердикт не выносили — перепроверяют).
#[tokio::test]
async fn clearing_lag_wakes_checks() {
    let lag = EventsLag::default();
    let ready = std::sync::Arc::new(tokio::sync::Notify::new());
    lag.on_clear(ready.clone());
    let waiter = ready.notified();
    tokio::pin!(waiter);
    waiter.as_mut().enable();
    lag.set(45);
    lag.set(0);
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("снятие отметки будит");
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
