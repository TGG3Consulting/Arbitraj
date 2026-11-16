//! Файл: tests/gateway_core_out_must_333.rs
//! Что делает: решения 333/334 и ревью 333 — очередь «ждут места» сообщений адаптеров ядру (gateway/core_out_must*.rs)
//! и журнал о потерях (gateway/lost_log.rs). Это НЕ запись площадки: данных площадок здесь нет и они не имитируются —
//! проверяется чистая логика очереди на явных значениях (метки порядка в тексте Notice и в причине ожидания CoinCheck,
//! очередь кошелька «свободна» — род Money) на настоящем канале tokio малой ёмкости и настоящем хранилище настроек и
//! журнале (SQLite во временной папке).
//! - Info (решение 362, в логику): без настройки ждут места по порядку; предел venue.info_events_max — сверх него
//!   потеряно с записью venue.core_events_lost (1-я, 2-я, 4-я потеря).
//! - Money (Б1): не теряется без всякой настройки; идёт раньше ждущих Info (Н1).
//! - CoinCheck (Б2): send_in_task — «не проверена» приходит ядру раньше итога той же задачи.
//! - Доставщик при закрытом канале (Н10): очереди очищены, признак «работает» снят, запись venue.core_events_queue.
//! - Н9: денежное событие ждало дольше venue.money_events_alert_ms — уведомление venue.money_event_delayed.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{core_out_must, lost_log}, arb::settings, arb::storage, tokio.
//! Настройки: venue.info_events_max, venue.money_events_alert_ms (задаются тестом в своей
//! временной базе).

use std::time::Duration;

use arb::core::types::{Asset, CheckState, CoinCheck, VenueId, WalletQueue, WalletQueueState};
use arb::gateway::core_out_must::{
    ALERT_MS, DELAYED_CODE, Fed, INFO_MAX, Kind, LOST_KIND, MustOut, QUEUE_KIND,
};
use arb::gateway::events::MarketEvent;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::{Value, json};
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

/// Задаёт настройку на всю систему.
async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест")
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

/// Проверка монеты с меткой в причине ожидания.
fn check(state: CheckState<arb::core::types::TokenTaxes>, tag: &str) -> MarketEvent {
    MarketEvent::CoinCheck(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "T".into(),
            bsc_contract: None,
        },
        taxes: state,
        source: "тест".into(),
        checked_at: None,
        wait: Some(tag.into()),
    })
}

/// Метка события для сравнения порядка.
fn tag(ev: &MarketEvent) -> String {
    match ev {
        MarketEvent::Notice { code, .. } if code == DELAYED_CODE => "delayed".into(),
        MarketEvent::Notice { text, .. } => text.clone(),
        MarketEvent::WalletQueue(_) => "queue".into(),
        MarketEvent::CoinCheck(c) => c.wait.clone().unwrap_or_default(),
        _ => "?".into(),
    }
}

/// Принимает n событий и отдаёт их метки.
async fn take(rx: &mut mpsc::Receiver<MarketEvent>, n: usize) -> Vec<String> {
    let mut got = Vec::new();
    for _ in 0..n {
        got.push(tag(&rx.recv().await.expect("событие")));
    }
    got
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

/// Решение 362: уведомления ждут места без всякой настройки, по порядку, без записей о потере.
#[tokio::test]
async fn info_waits_and_keeps_order() {
    let (s, j) = fresh("order").await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx, &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    assert_eq!(out.send(notice("b")), Fed::Kept);
    assert_eq!(out.send(notice("c")), Fed::Kept);
    assert_eq!(take(&mut rx, 3).await, ["a", "b", "c"]);
    assert_eq!(logged(&j, LOST_KIND, 0).await, 0);
}

/// Решение 362: потеря уведомления — только сверх предела venue.info_events_max, и не молча: запись журнала на 1-й,
/// 2-й, 4-й потере (3-я — нет); ждущее до предела доставлено.
#[tokio::test]
async fn info_over_max_is_lost_but_logged() {
    let (s, j) = fresh("lost").await;
    set(&s, INFO_MAX, json!(1)).await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx, &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    assert_eq!(out.send(notice("b")), Fed::Kept);
    for _ in 0..4 {
        assert_eq!(out.send(notice("x")), Fed::Lost);
    }
    assert_eq!(take(&mut rx, 2).await, ["a", "b"]);
    assert_eq!(logged(&j, LOST_KIND, 3).await, 3);
    assert_eq!(out.send(notice("c")), Fed::Sent);
}

#[tokio::test]
async fn info_max_limits_queue() {
    let (s, j) = fresh("max").await;
    set(&s, INFO_MAX, json!(1)).await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx, &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    assert_eq!(out.send(notice("b")), Fed::Kept);
    assert_eq!(out.send(notice("c")), Fed::Lost);
    assert_eq!(take(&mut rx, 2).await, ["a", "b"]);
}

#[tokio::test]
async fn money_never_lost_and_goes_first() {
    let (s, j) = fresh("money").await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx.clone(), &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    // Б1: ни одной настройки — канал полон, Money ждёт места.
    assert_eq!(out.send(queue_free()), Fed::Kept);
    assert_eq!(take(&mut rx, 2).await, ["a", "queue"]);
    // Н1: Money впереди ждущих Info (всё поставлено до первого await — доставщик ещё не брал).
    assert_eq!(out.send(notice("b")), Fed::Sent);
    assert_eq!(out.send(notice("c")), Fed::Kept);
    assert_eq!(out.send(notice("d")), Fed::Kept);
    assert_eq!(out.send(queue_free()), Fed::Kept);
    assert_eq!(take(&mut rx, 4).await, ["b", "queue", "c", "d"]);
    assert_eq!(Kind::of(&queue_free()), Kind::Money);
    assert_eq!(Kind::of(&notice("x")), Kind::Info);
}

#[tokio::test]
async fn coin_check_in_task_keeps_order() {
    let (s, j) = fresh("coin").await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx.clone(), &s, &j);
    assert_eq!(out.send(notice("x")), Fed::Sent);
    // Доставщик уже ждёт место для «y».
    assert_eq!(out.send(notice("y")), Fed::Kept);
    let task = tokio::spawn(async move {
        let fed = out
            .send_in_task(check(CheckState::NotChecked, "не проверена"))
            .await;
        let fail = CheckState::Failed {
            reason: "итог".into(),
        };
        out.out().send(check(fail, "итог")).await.expect("канал");
        fed
    });
    let got = take(&mut rx, 4).await;
    assert_eq!(task.await.expect("задача"), Fed::Sent);
    let pos = |t: &str| got.iter().position(|g| g == t).expect("метка");
    assert!(pos("не проверена") < pos("итог"), "{got:?}");
}

#[tokio::test]
async fn deliverer_clears_on_closed_channel() {
    let (s, j) = fresh("closed").await;
    let (tx, rx) = mpsc::channel(1);
    let out = MustOut::new(tx, &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    assert_eq!(out.send(notice("b")), Fed::Kept);
    assert_eq!(out.send(queue_free()), Fed::Kept);
    drop(rx);
    let q = out.queue();
    let mut snap = q.snapshot();
    for _ in 0..100 {
        snap = q.snapshot();
        if !snap.running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!((snap.money, snap.info, snap.running), (0, 0, false));
    assert_eq!(out.send(queue_free()), Fed::Closed);
    assert!(logged(&j, QUEUE_KIND, 1).await >= 1);
}

#[tokio::test]
async fn money_delay_alert() {
    let (s, j) = fresh("alert").await;
    set(&s, ALERT_MS, json!(1)).await;
    let (tx, mut rx) = mpsc::channel(1);
    let out = MustOut::new(tx, &s, &j);
    assert_eq!(out.send(notice("a")), Fed::Sent);
    assert_eq!(out.send(queue_free()), Fed::Kept);
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(take(&mut rx, 3).await, ["a", "queue", "delayed"]);
}
