//! Файл: tests/notify_queue_340.rs
//! Что делает: решение 340 (задача t158) — уведомления уходят фоновой очередью (notify/queue.rs): порядок уведомлений
//! одной сделки сохраняется; send возвращается сразу, даже когда очередь занята медленным заданием; при заданной
//! notify.queue_max полная очередь даёт потерю со счётом и записью журнала notify.queue_lost; снимок карточки сделки
//! в задании сильнее свежей карточки. Настоящий рантайм tokio, настоящая SQLite во временной папке, без Telegram
//! (чат не задан — итог каждой отправки — запись журнала «не отправлено»), без моков.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::notify, arb::settings, arb::storage, arb::secrets.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use arb::notify::alert_card::{self, DealCard, Snap};
use arb::notify::catalog;
use arb::notify::dispatch::Notifier;
use arb::notify::queue;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal};
use serde_json::json;
use tokio::sync::broadcast;

/// Отправитель уведомлений на базе во временной папке (Telegram не настроен).
async fn notifier(name: &str) -> Notifier {
    let d: PathBuf = std::env::temp_dir().join(format!("arb-q340-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Notifier {
        http: reqwest::Client::new(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("s.enc")),
        journal,
    }
}

/// Включает событие (область «событие»).
async fn enable(n: &Notifier, code: &str) {
    n.settings
        .set(
            "notify.event.enabled",
            Scope::Event(code.into()),
            Some(json!(true)),
            "тест",
        )
        .await
        .unwrap();
}

/// Записи журнала вида kind по порядку записи (старые первыми) — после того, как очередь опустела.
async fn of_kind(n: &Notifier, kind: &str) -> Vec<Entry> {
    queue::idle().await;
    let mut all: Vec<Entry> = n.journal.list(1000, None).await.unwrap();
    all.retain(|e| e.kind == kind);
    all.sort_by_key(|e| e.id);
    all
}

/// Четыре события сделки по порядку ТЗ: куплено → переведено → зачислено → продано.
const DEAL: [&str; 4] = [
    "deal.buy_filled",
    "deal.transfer_sent",
    "deal.transfer_credited",
    "deal.sell_filled",
];

#[tokio::test]
async fn deal_notices_keep_order() {
    let n = notifier("order").await;
    for c in DEAL {
        enable(&n, c).await;
    }
    for c in DEAL {
        n.send(c, Some("bingx"), "Сделка: 20261116120000000CAKE")
            .await;
    }
    let got = of_kind(&n, "notify").await;
    assert_eq!(got.len(), DEAL.len(), "{got:?}");
    for (e, c) in got.iter().zip(DEAL) {
        let title = catalog::find(c).unwrap().title;
        assert!(
            e.message.starts_with(&format!("«{title}» не отправлено")),
            "{c}: {}",
            e.message
        );
    }
}

#[tokio::test]
async fn send_does_not_wait_slow_queue() {
    let n = notifier("slow").await;
    enable(&n, "deal.buy_filled").await;
    // Медленная отправка — задание очереди, которое держит её 1 с (как Telegram, не ответивший до таймаута).
    let slow = Duration::from_secs(1);
    n.later("deal.closed", None, Snap::default(), async move {
        tokio::time::sleep(slow).await;
    });
    let t0 = Instant::now();
    n.send("deal.buy_filled", Some("bingx"), "Куплено").await;
    let step = t0.elapsed();
    assert!(
        step < Duration::from_millis(200),
        "шаг сделки ждал {step:?}"
    );
    // Уведомление ушло после медленного (порядок) — запись появляется только когда очередь освободилась.
    let got = of_kind(&n, "notify").await;
    assert_eq!(got.len(), 1);
    assert!(t0.elapsed() >= slow, "{:?}", t0.elapsed());
}

#[tokio::test]
async fn full_queue_loses_with_record() {
    let n = notifier("full").await;
    enable(&n, "deal.buy_filled").await;
    n.settings
        .set("notify.queue_max", Scope::Global, Some(json!(1)), "тест")
        .await
        .unwrap();
    let before = queue::lost_total();
    n.later("deal.closed", None, Snap::default(), async {
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
    n.send("deal.buy_filled", Some("bingx"), "Куплено").await;
    assert_eq!(queue::lost_total(), before + 1, "потеря посчитана");
    // Запись о потере пишется своей задачей — ждём её появления.
    let mut lost = vec![];
    for _ in 0..100 {
        lost = of_kind(&n, queue::LOST_KIND).await;
        if !lost.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(lost.len(), 1, "запись о потере: {lost:?}");
    assert!(lost[0].message.contains("deal.buy_filled"));
    // Отправки потерянного не было.
    assert!(of_kind(&n, "notify").await.is_empty());
}

#[tokio::test]
async fn snapshot_card_wins_over_fresh() {
    let fresh = DealCard {
        coin: "CAKE".into(),
        path: "BingX → DEX".into(),
        now: "в пути".into(),
    };
    let at_event = DealCard {
        now: "монета: 10 CAKE на BingX".into(),
        ..fresh.clone()
    };
    let f = fresh.clone();
    alert_card::install(move |id| (id == "D1").then(|| f.clone()));
    let snap = Snap::of("D1", Some(at_event.clone()));
    let got = alert_card::scoped(snap, async { alert_card::at_event("D1") }).await;
    assert_eq!(got, Some(at_event), "в задании — снимок на момент события");
    assert_eq!(
        alert_card::at_event("D1"),
        Some(fresh),
        "без снимка — свежая"
    );
    // Снимок другой сделки не подменяет карточку этой.
    let other = Snap::of("D2", None);
    let got = alert_card::scoped(other, async { alert_card::at_event("D1") }).await;
    assert_eq!(got.map(|c| c.now), Some("в пути".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn risk_alert_not_behind_slow_info() {
    // Решение 356 (ревью t158 Н1): алерт риска — своя полоса, не ждёт обычные уведомления (Telegram лежит).
    let n = notifier("lanes").await;
    enable(&n, "risk.stop").await;
    let slow = Duration::from_secs(2);
    n.later("deal.closed", None, Snap::default(), async move {
        tokio::time::sleep(slow).await;
    });
    let t0 = Instant::now();
    n.send("risk.stop", None, "Стоп по риску").await;
    let title = catalog::find("risk.stop").unwrap().title;
    loop {
        let all: Vec<Entry> = n.journal.list(1000, None).await.unwrap();
        if all
            .iter()
            .any(|e| e.kind == "notify" && e.message.starts_with(&format!("«{title}»")))
        {
            break;
        }
        assert!(t0.elapsed() < slow, "алерт ждал обычную полосу");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
