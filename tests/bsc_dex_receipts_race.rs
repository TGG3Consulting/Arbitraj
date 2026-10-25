//! Файл: tests/bsc_dex_receipts_race.rs
//! Что делает: гонка повтора блока ленты квитанций на все узлы (решение 71, ревью #61): побеждает первый ПОЛНЫЙ ответ,
//! а не первый HTTP 200 — null и неполные квитанции отстающего узла не выигрывают. Ответы — живая batch-запись
//! публичного узла (tests/data/receipts_batch_2026-10-25/responses.json).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node::{rpc_race, rpc_capped, receipts_fetch, receipts_chain}.

use serde_json::Value;

use arb::venues::bsc_dex::node::receipts_chain::complete;
use arb::venues::bsc_dex::node::receipts_fetch::pair;

fn batch_record() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string("tests/data/receipts_batch_2026-10-25/responses.json").unwrap(),
    )
    .unwrap()
}

/// Ревью #61: гонка повтора на все узлы берёт первый ПОЛНЫЙ ответ, а не первый HTTP 200. Отстающий узел быстро
/// отвечает null (блока ещё нет) или неполными квитанциями — побеждает пришедший позже полный ответ. Ответы — живая
/// batch-запись блока 129761640 публичного узла; null и неполный — из неё же (убран заголовок, убрана квитанция).
#[tokio::test]
async fn race_takes_first_complete_answer_not_first_200() {
    use arb::venues::bsc_dex::node::rpc_capped::Capped;
    use arb::venues::bsc_dex::node::rpc_race::first_accepted;
    use std::time::Duration;
    let rec = batch_record();
    let r = &rec["results"][1];
    let n = u64::from_str_radix(
        r["request"][0]["params"][0]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let full = r["response"].clone();
    let mut null = full.clone();
    null[0]["result"] = Value::Null;
    let mut short = full.clone();
    short[1]["result"].as_array_mut().unwrap().pop();
    let reject = |why: String| arb::venues::bsc_dex::node::rpc_race::Reject { why, head: None };
    let accept = |list| {
        let (h, receipts) = pair(n, list).map_err(reject)?;
        complete(n, h.hash, &h.transactions, &receipts).map_err(reject)?;
        Ok(receipts.len())
    };
    let answer = |v: Value, ms: u64| async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(Capped::Got(v.to_string()))
    };
    let want = full[1]["result"].as_array().unwrap().len();
    for bad in [null, short] {
        let runs = vec![
            Box::pin(answer(bad.clone(), 0)),
            Box::pin(answer(full.clone(), 30)),
        ];
        let got = first_accepted(runs, 2, accept).await;
        assert!(matches!(got, Ok(k) if k == want), "побеждает полный ответ");
    }
    // Оба неполные — гонка проиграна с причиной (повтор ленты), а не «успех».
    let mut short2 = full.clone();
    short2[1]["result"].as_array_mut().unwrap().pop();
    let runs = vec![
        Box::pin(answer(short2.clone(), 0)),
        Box::pin(answer(short2, 10)),
    ];
    assert!(first_accepted(runs, 2, accept).await.is_err());
}
