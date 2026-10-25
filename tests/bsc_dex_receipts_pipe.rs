//! Файл: tests/bsc_dex_receipts_pipe.rs
//! Что делает: конвейер ленты квитанций (решение 69) на ЖИВЫХ записях узла BNB Chain:
//! - заголовок и квитанции блока одним JSON-RPC batch — разбор настоящего ответа публичного узла
//!   (tests/data/receipts_batch_2026-10-25/responses.json: блоки 129761639–129761641 по одному и два блока одним
//!   POST): итоги по id (и в обратном порядке), квитанции полные по заголовку; ошибка одного вызова внутри пачки —
//!   итог этого вызова, блок не принят;
//! - конвейер (node/receipts_pipe.rs) на блоках окна tests/data/receipts_2026-10-25/responses.json: следующие блоки
//!   запрашиваются вперёд, отдаются строго по порядку; блок, который не пришёл с первого раза, запрашивается снова, а
//!   пришедшие после него ждут в конвейере и повторно не запрашиваются;
//! - на каком блоке и почему конвейер стоит (решение 71) — для записи при обрыве и гонки повтора на все узлы;
//! - гонка повтора берёт первый ПОЛНЫЙ ответ (null и неполные квитанции не выигрывают).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/receipts_common, arb::venues::bsc_dex::node::{rpc_capped, receipts_io, receipts_chain,
//! receipts_pipe}.

mod receipts_common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use arb::venues::bsc_dex::node::receipts_chain::complete;
use arb::venues::bsc_dex::node::receipts_fetch::pair;
use arb::venues::bsc_dex::node::receipts_pipe::Pipe;
use arb::venues::bsc_dex::node::rpc_capped::split;
use receipts_common::{hdr, load, result};

fn batch_record() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string("tests/data/receipts_batch_2026-10-25/responses.json").unwrap(),
    )
    .unwrap()
}

#[test]
fn header_and_receipts_in_one_batch() {
    let rec = batch_record();
    for r in rec["results"].as_array().unwrap() {
        let text = r["response"].to_string();
        let calls = r["request"].as_array().unwrap().len();
        let list = split(&text, calls).unwrap();
        assert_eq!(list.len(), calls);
        // Пары «заголовок, квитанции» по порядку вызовов (id 0–1, 2–3).
        let mut it = list.into_iter();
        for k in 0..calls / 2 {
            let n = u64::from_str_radix(
                r["request"][2 * k]["params"][0]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap();
            let (h, receipts) = pair(n, vec![it.next().unwrap(), it.next().unwrap()]).unwrap();
            assert_eq!(
                complete(n, h.hash, &h.transactions, &receipts),
                Ok(()),
                "{}",
                r["name"]
            );
            assert!(!receipts.is_empty());
        }
    }
    // Ответы в обратном порядке — итоги всё равно по id (JSON-RPC 2.0, раздел 6).
    let first = &rec["results"][0];
    let n = u64::from_str_radix(
        first["request"][0]["params"][0]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let mut rev = first["response"].as_array().unwrap().clone();
    rev.reverse();
    let (h, receipts) = pair(n, split(&Value::Array(rev).to_string(), 2).unwrap()).unwrap();
    assert_eq!(complete(n, h.hash, &h.transactions, &receipts), Ok(()));
    // Ошибка одного вызова внутри пачки (узел ответил HTTP 200, у квитанций — error) — итог этого вызова: блок не
    // принят, причина с текстом узла (лента повторит через bsc.receipts_retry_ms).
    let mut bad = first["response"].as_array().unwrap().clone();
    bad[1] = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32005, "message": "limit exceeded" } });
    let list = split(&Value::Array(bad).to_string(), 2).unwrap();
    assert!(list[0].is_ok() && list[1].is_err());
    let why = pair(n, list).unwrap_err();
    assert!(why.contains("limit exceeded"), "{why}");
    // Ответа на какой-то id нет — ошибка пачки, а не тихий пропуск.
    let one = rec["results"][0]["response"].as_array().unwrap()[..1].to_vec();
    assert!(split(&Value::Array(one).to_string(), 2).is_err());
}

#[tokio::test]
async fn pipe_hands_blocks_in_order_and_failed_block_holds_the_rest() {
    let rec = load("responses.json");
    let window: Vec<u64> = rec["window"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    // «Узел» — записанные блоки окна; второй блок окна с первого раза не приходит (сбой узла).
    let data: HashMap<u64, usize> = window
        .iter()
        .map(|&n| {
            (
                n,
                result(&rec, &format!("receipts {n}"))
                    .as_array()
                    .unwrap()
                    .len(),
            )
        })
        .collect();
    let data = Arc::new(data);
    let calls: Arc<Mutex<HashMap<u64, usize>>> = Arc::default();
    let failed_once = Arc::new(AtomicUsize::new(0));
    let flaky = window[1];
    let fetch = |n: u64| {
        let (data, calls, failed_once) = (data.clone(), calls.clone(), failed_once.clone());
        async move {
            *calls.lock().unwrap().entry(n).or_default() += 1;
            if n == flaky && failed_once.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(format!("блок {n}: нет связи"));
            }
            Ok((n, data[&n]))
        }
    };
    let mut pipe: Pipe<Result<(u64, usize), String>> = Pipe::default();
    let (mut top, last) = (window[0] - 1, *window.last().unwrap());
    let mut order = Vec::new();
    let mut held = None;
    while top < last {
        let n = top + 1;
        pipe.fill(n, last, window.len() as u64, fetch);
        match pipe.take(n).await.unwrap() {
            Ok((got, receipts)) => {
                assert_eq!(got, n);
                assert_eq!(
                    receipts,
                    hdr(&rec, n).2.len(),
                    "квитанций столько же, сколько транзакций"
                );
                order.push(got);
                top = n;
            }
            Err(_) => {
                // Не пришёл — следующие ждут в конвейере (не теряются и не обгоняют его).
                held = Some(pipe.len());
            }
        }
    }
    assert_eq!(order, window, "строго по порядку");
    assert_eq!(
        held,
        Some(window.len() - 2),
        "после сбоя второго блока в пути — следующие"
    );
    let calls = calls.lock().unwrap();
    for &n in &window {
        let want = if n == flaky { 2 } else { 1 };
        assert_eq!(
            calls[&n], want,
            "блок {n}: повторно запрашивается только не пришедший"
        );
    }
    assert!(pipe.is_empty());
}

/// Решение 71: на каком блоке и почему стоит конвейер — для записи при обрыве по отставанию; этот блок повторяется
/// гонкой на все узлы. Причина — как её пишет лента (блок 129767716 живого бота 25.10: 2 073 166 байт дольше
/// bsc.receipts_total_ms = 5000).
#[test]
fn lag_names_the_block_the_pipe_stands_on() {
    use arb::venues::bsc_dex::node::receipts_lag::Lag;
    let mut lag = Lag::default();
    assert_eq!(lag.hot(), None);
    assert!(lag.stuck_text().is_empty());
    let why = "блок 129767716: квитанции больше предела bsc.receipts_total_ms (5000 мс), получено 2073166 байт";
    lag.failed(129767716, why);
    lag.failed(129767716, why);
    assert_eq!(
        lag.hot(),
        Some(129767716),
        "повтор этого блока — гонкой на все узлы"
    );
    let text = lag.stuck_text();
    assert!(
        text.contains("блоке 129767716")
            && text.contains("повторов 2")
            && text.contains("2073166 байт"),
        "{text}"
    );
    // Следующий блок разобран — конвейер больше не стоит.
    lag.passed(129767716);
    assert_eq!(lag.hot(), None);
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
    let accept = |list| {
        let (h, receipts) = pair(n, list)?;
        complete(n, h.hash, &h.transactions, &receipts)?;
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
