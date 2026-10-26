//! Файл: tests/bsc_dex_receipts_reset.rs
//! Что делает: полный сброс ленты квитанций не открывает окно торговли по старым данным (ревью #62):
//! - после сброса (Feed::reset) котировка и сделка запрещены (StaleData), даже когда новая лента сняла отставание;
//! - запрет снимается, только когда закрыты все «соединения» пачек старой ленты (пачка закрывает своё после
//!   Invalidated — pools/session.rs); «соединения» новой ленты не мешают;
//! - снятие из-за сброса циклы проверок не будит;
//! - смена источника receipts → ws (Feed::stop_source): отставание обнуляется, запрет держится, пока открыты
//!   «соединения» пачек старой ленты, и снимается последним из них — при ws котировки снова есть; обратно ws → receipts —
//!   отметка новой ленты ставится заново, котировок нет до догона;
//! - один узел (нет ни списка, ни резервного): «блока нет» с головой не ниже блока — невосполнимо только после двух
//!   таких ответов подряд (живая batch-запись блока 129761640, заголовок убран); сбой между ними обнуляет счёт.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common (окружение на настоящей SQLite), arb::venues::bsc_dex::node::{events_lag,
//! receipts, receipts_fetch, receipts_lag, rpc_race}.

mod native_common;

use std::sync::Arc;
use std::time::Duration;

use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::node::events_lag::EventsLag;
use arb::venues::bsc_dex::node::receipts::Feed;
use arb::venues::bsc_dex::node::receipts_fetch::{lost, pair};
use arb::venues::bsc_dex::node::receipts_io::Fail;
use arb::venues::bsc_dex::node::receipts_lag::Lag;
use arb::venues::bsc_dex::node::rpc_capped::Capped;
use arb::venues::bsc_dex::node::rpc_race::{Reject, first_accepted};
use serde_json::Value;

#[tokio::test]
async fn reset_keeps_mark_until_old_links_closed() {
    let ctx = native_common::ctx("receipts-reset").await;
    let lag = EventsLag::default();
    let ready = Arc::new(tokio::sync::Notify::new());
    lag.on_clear(ready.clone());
    let feed = Feed::with_lag(lag.clone());
    let old = feed.open_idle();
    // Лента стояла и отстала — сброс.
    lag.set(130);
    feed.reset("блок стоит дольше bsc.receipts_stuck_reset_s");
    let woke = ready.notified();
    tokio::pin!(woke);
    woke.as_mut().enable();
    let e = lag.check(&ctx).unwrap_err();
    assert_eq!(e.code, ErrorCode::StaleData);
    // Новая лента пошла с текущего блока и сняла отставание — запрет по сбросу стоит, пока пачка старой не закрылась.
    lag.set(0);
    assert!(lag.check(&ctx).is_err(), "до Invalidated котировок нет");
    assert!(lag.busy(), "проверки вердикт не выносят");
    let new = feed.open_idle();
    assert!(
        !feed.release_if_settled(),
        "старое «соединение» ещё открыто"
    );
    assert!(lag.check(&ctx).is_err());
    drop(old);
    assert!(feed.release_if_settled(), "все старые закрыты — снимается");
    assert!(lag.check(&ctx).is_ok());
    assert!(!lag.busy());
    assert!(!feed.release_if_settled(), "снимается один раз");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), woke)
            .await
            .is_err(),
        "снятие из-за сброса циклы не будит"
    );
    drop(new);
}

/// Ревью #62: при одном узле одно «нет» может быть сбоем за балансировщиком — Missing несёт число узлов (1), лента
/// считает такие ответы подряд (Lag::nowhere) и рвёт ленту только со второго; сбой другого рода между ними — счёт с нуля.
#[tokio::test]
async fn single_node_needs_two_nowhere_in_a_row() {
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/receipts_batch_2026-10-25/responses.json").unwrap(),
    )
    .unwrap();
    let r = &rec["results"][1];
    let tag = r["request"][0]["params"][0].as_str().unwrap();
    let n = u64::from_str_radix(tag.trim_start_matches("0x"), 16).unwrap();
    let mut null = r["response"].clone();
    null[0]["result"] = Value::Null;
    null.as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "result": format!("0x{:x}", n + 1) }));
    let accept =
        |list: Vec<arb::gateway::errors::GatewayResult<Box<serde_json::value::RawValue>>>| {
            let head = Some(n + 1);
            pair(n, list)
                .map(|_| ())
                .map_err(|why| Reject { why, head })
        };
    let one = vec![async { Ok(Capped::Got(null.to_string())) }];
    let l = first_accepted(one, 3, accept).await.unwrap_err();
    let lim = arb::gateway::http::Limits {
        head: Duration::from_secs(1),
        idle: Duration::from_secs(1),
        total: Duration::from_secs(1),
        max_bytes: 1,
    };
    let Fail::Missing(why, nodes) = lost(n, l, &lim, true) else {
        panic!("один узел дошёл до блока и не отдаёт — «нет нигде»");
    };
    assert_eq!(nodes, 1);
    let mut lag = Lag::default();
    assert_eq!(lag.nowhere(n), 0, "первое «нет» — ещё повтор");
    lag.failed(n, &why, true);
    assert_eq!(lag.nowhere(n), 1, "второе подряд — невосполнимо");
    lag.failed(n, "узел не ответил", false);
    assert_eq!(lag.nowhere(n), 0, "сбой между ними — счёт заново");
    assert_eq!(lag.nowhere(n + 1), 0, "счёт — по своему блоку");
}

/// Повторное ревью #62: отметка одна на площадку, а снять её может только лента — при переключении на ws она
/// остановлена. Теперь остановка обнуляет отставание и держит запрет только до закрытия «соединений» пачек (они
/// закрывают их после Invalidated своих пулов), затем котировки по WS разрешены.
#[tokio::test]
async fn switch_to_ws_releases_mark_after_links_closed() {
    let ctx = native_common::ctx("receipts-switch").await;
    let lag = EventsLag::default();
    let feed = Feed::with_lag(lag.clone());
    let pack = feed.open_named("пачка ZKP");
    // Лента отстала и была сброшена.
    lag.set(60);
    feed.reset("блок стоит дольше bsc.receipts_stuck_reset_s");
    assert!(lag.check(&ctx).is_err());
    assert_eq!(
        feed.holders(),
        vec!["пачка ZKP".to_string()],
        "запрет держит эта пачка"
    );
    // Источник сменён на ws: отставание обнулено, запрет — до закрытия «соединения» пачки.
    feed.stop_source("источник событий пулов сменён (bsc.pool_events_source) — лента остановлена");
    assert_eq!(lag.get(), 0);
    assert!(
        lag.check(&ctx).is_err(),
        "пачка ещё не сделала пулы недостоверными"
    );
    drop(pack);
    assert!(
        lag.check(&ctx).is_ok(),
        "пачки переподключились по ws — котировки есть"
    );
    assert!(!lag.busy());
    // Обратно на receipts: новая лента отстаёт больше предела — отметка заново, котировок нет до догона.
    let mut mark = Lag::default();
    mark.mark(&ctx, &lag, 50, 40).await;
    assert!(lag.check(&ctx).is_err());
    mark.mark(&ctx, &lag, 3, 40).await;
    assert!(lag.check(&ctx).is_ok(), "догнала — котировки есть");
}

/// Остановка ленты без открытых «соединений» — запрет снимается сразу.
#[tokio::test]
async fn switch_without_links_releases_at_once() {
    let ctx = native_common::ctx("receipts-switch-empty").await;
    let lag = EventsLag::default();
    let feed = Feed::with_lag(lag.clone());
    lag.set(45);
    feed.stop_source("источник сменён");
    assert!(lag.check(&ctx).is_ok());
}
