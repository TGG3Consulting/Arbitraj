//! Файл: tests/gateway_core_out_must_333.rs
//! Что делает: решения 333/334 — очередь «ждут места» сообщений адаптеров ядру (gateway/core_out_must.rs) и запись
//! журнала о потерях (gateway/lost_log.rs) на настоящем канале tokio малой ёмкости и настоящем хранилище настроек и
//! журнале (SQLite во временной папке). Данных площадок нет: проверяется чистая логика очереди, сообщения — уведомления
//! (MarketEvent::Notice, текст — метка порядка) и очередь кошелька «свободна» (WalletQueue — род Money).
//! - venue.info_events_keep = да: канал полон — уведомления ждут места и уходят ядру по порядку;
//! - не задана — как раньше потеряно, но с записью журнала venue.core_events_lost;
//! - очередь не пуста — новое встаёт в конец при любой настройке (не обгоняет ждущие);
//! - род Money отдельно: «да» у Info не держит WalletQueue, venue.money_events_keep = да — держит;
//! - журнал потерь без интервала — на 1-й, 2-й, 4-й… потере.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{core_out_must, lost_log}, arb::settings, arb::storage, tokio.
//! Настройки: venue.info_events_keep, venue.money_events_keep (задаются тестом в своей временной базе).

use std::time::Duration;

use arb::core::types::{VenueId, WalletQueue, WalletQueueState};
use arb::gateway::core_out_must::{Fed, KEEP_INFO, KEEP_MONEY, LOST_KIND, MustOut};
use arb::gateway::events::MarketEvent;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

/// Новая база во временной папке: настройки и журнал. Ошибок нет (паника теста).
async fn fresh(name: &str) -> (Store, Journal) {
    let dir = std::env::temp_dir().join(format!("arb-t333-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("папка");
    let db = Db::open(&dir.join("t.sqlite")).expect("база");
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    let journal = Journal::new(db.clone(), jtx);
    let store = Store::load(db, tx, journal.clone())
        .await
        .expect("настройки");
    (store, journal)
}

/// Включает настройку «да» на всю систему.
async fn on(s: &Store, name: &str) {
    s.set(name, Scope::Global, Some(json!(true)), "тест")
        .await
        .expect("настройка");
}

/// Уведомление-метка.
fn notice(text: &str) -> MarketEvent {
    MarketEvent::Notice {
        venue: VenueId("bsc_dex".into()),
        code: "test".into(),
        text: text.into(),
    }
}

/// Очередь кошелька «свободна» (род Money).
fn queue_free() -> MarketEvent {
    MarketEvent::WalletQueue(WalletQueue {
        venue: VenueId("bsc_dex".into()),
        account: None,
        state: WalletQueueState::Free,
        nonce_taken: None,
    })
}

/// Метка события для сравнения порядка.
fn tag(ev: &MarketEvent) -> String {
    match ev {
        MarketEvent::Notice { text, .. } => text.clone(),
        MarketEvent::WalletQueue(_) => "queue".into(),
        _ => "?".into(),
    }
}

/// Сколько записей журнала вида kind (запись пишется отдельной задачей — ждём до секунды).
async fn logged(j: &Journal, kind: &str, want: usize) -> usize {
    let mut n = 0;
    for _ in 0..100 {
        n = j
            .list(500, None)
            .await
            .expect("журнал")
            .iter()
            .filter(|e| e.kind == kind)
            .count();
        if n >= want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    n
}

#[tokio::test]
async fn info_keep_waits_and_keeps_order() {
    let (s, j) = fresh("order").await;
    on(&s, KEEP_INFO).await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx, &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    assert_eq!(out.send(notice("b")), Fed::Kept);
    assert_eq!(out.send(notice("c")), Fed::Kept);
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(tag(&rx.recv().await.expect("событие")));
    }
    assert_eq!(got, ["a", "b", "c"]);
    assert_eq!(logged(&j, LOST_KIND, 0).await, 0);
}

#[tokio::test]
async fn unset_is_lost_but_logged() {
    let (s, j) = fresh("lost").await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx, &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    // 1-я, 2-я, 4-я потеря — запись журнала; 3-я — нет.
    for _ in 0..4 {
        assert_eq!(out.send(notice("x")), Fed::Lost);
    }
    assert_eq!(tag(&rx.recv().await.expect("событие")), "a");
    assert_eq!(logged(&j, LOST_KIND, 3).await, 3);
    // Место есть — снова без ожидания.
    assert_eq!(out.send(notice("b")), Fed::Sent);
}

#[tokio::test]
async fn money_kind_has_own_setting_and_queue_order_holds() {
    let (s, j) = fresh("money").await;
    on(&s, KEEP_INFO).await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx.clone(), &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    // Канал полон, очередь пуста: Money без своей настройки потеряна.
    assert_eq!(out.send(queue_free()), Fed::Lost);
    // Info ждёт; пока очередь не пуста, Money встаёт за ней (порядок), хотя её настройка не «да».
    assert_eq!(out.send(notice("b")), Fed::Kept);
    assert_eq!(out.send(queue_free()), Fed::Kept);
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(tag(&rx.recv().await.expect("событие")));
    }
    assert_eq!(got, ["a", "b", "queue"]);
    // Своя настройка — Money ждёт места при пустой очереди.
    on(&s, KEEP_MONEY).await;
    let other = MustOut::new(tx, &s, &j);
    assert_eq!(other.send(notice("c")), Fed::Sent);
    assert_eq!(other.send(queue_free()), Fed::Kept);
    assert_eq!(tag(&rx.recv().await.expect("событие")), "c");
    assert_eq!(tag(&rx.recv().await.expect("событие")), "queue");
}
