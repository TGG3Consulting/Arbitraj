//! Файл: tests/journal_queue_341.rs
//! Что делает: решение 341 (задача t158; по решению владельца — всегда) — очередь записи журнала
//! (storage/journal_queue.rs): write возвращается сразу (id = 0), чтение дожидается записанного до него; поток-писатель
//! пишет по порядку постановки (пачками, в том числе по одной записи — journal.batch_max = 1), живой поток получает
//! записи с настоящим id; запись не ждёт основную базу, занятую долгой транзакцией; предел journal.queue_max — полная
//! очередь даёт потерю со счётом и записью journal.queue_lost. Настоящая SQLite во временной папке, без моков.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::storage (db, journal, journal_queue).

use std::time::{Duration, Instant};

use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal, Level};
use arb::storage::journal_queue::LOST_KIND;
use tokio::sync::broadcast;

/// Журнал на базе во временной папке и приёмник его живого потока.
fn journal(name: &str) -> (Journal, broadcast::Receiver<Entry>) {
    let d = std::env::temp_dir().join(format!("arb-j341-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (tx, rx) = broadcast::channel(1024);
    (Journal::new(db, tx), rx)
}

/// Записи вида kind, старые первыми (после того, как очередь опустела).
async fn of_kind(j: &Journal, kind: &str) -> Vec<Entry> {
    j.idle().await;
    let mut all = j.list(1000, None).await.unwrap();
    all.retain(|e| e.kind == kind);
    all.sort_by_key(|e| e.id);
    all
}

/// Держит замок основной базы (долгая транзакция, как длинная пачка) заданное время; возвращается, когда замок взят.
fn hold_main(j: &Journal, for_: Duration) -> tokio::task::JoinHandle<()> {
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let db = j.db().clone();
    let h = tokio::task::spawn_blocking(move || {
        db.call_sync(|c| {
            let t = c.transaction()?;
            held_tx.send(()).unwrap();
            std::thread::sleep(for_);
            t.commit()
        })
        .unwrap();
    });
    held_rx.recv().unwrap();
    h
}

#[tokio::test]
async fn list_sees_own_writes() {
    // Очередь всегда (решение владельца): запись не ждётся, а чтение дожидается записанного до него.
    let (j, _rx) = journal("rw");
    let e = j
        .write(Level::Info, "t.rw", None, "раз", None)
        .await
        .unwrap();
    assert_eq!(e.id, 0, "write не ждёт базу — id ещё нет");
    let got = j.list(10, None).await.unwrap();
    assert!(got.iter().any(|x| x.kind == "t.rw" && x.id > 0), "{got:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_write_keeps_order_and_does_not_wait() {
    let (j, mut rx) = journal("order");
    j.set_batch_max(Some(1));
    let hold = hold_main(&j, Duration::from_millis(800));
    let t0 = Instant::now();
    for i in 0..20 {
        let e = j
            .write(Level::Info, "t.order", None, format!("{i}"), None)
            .await
            .unwrap();
        assert_eq!(e.id, 0, "в очереди id ещё нет");
    }
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(300),
        "write ждал базу: {took:?}"
    );
    hold.await.unwrap();
    let got = of_kind(&j, "t.order").await;
    let texts: Vec<String> = got.iter().map(|e| e.message.clone()).collect();
    let want: Vec<String> = (0..20).map(|i| i.to_string()).collect();
    assert_eq!(texts, want, "порядок постановки");
    // Живой поток — те же записи, с настоящими id, по порядку.
    let mut live = vec![];
    while let Ok(e) = rx.try_recv() {
        if e.kind == "t.order" {
            live.push((e.id, e.message));
        }
    }
    let ids: Vec<(i64, String)> = got.iter().map(|e| (e.id, e.message.clone())).collect();
    assert_eq!(live, ids);
}

#[tokio::test(flavor = "multi_thread")]
async fn full_queue_loses_with_record() {
    let (j, _rx) = journal("full");
    j.set_queue_max(Some(1));
    let hold = hold_main(&j, Duration::from_millis(300));
    for i in 0..3 {
        let _ = j
            .write(Level::Info, "t.full", None, format!("{i}"), None)
            .await;
    }
    hold.await.unwrap();
    j.idle().await;
    // Следующая пачка дописывает запись о потерях.
    let _ = j.write(Level::Info, "t.full", None, "после", None).await;
    let lost = of_kind(&j, LOST_KIND).await;
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(lost[0].data.as_ref().unwrap()["total"], 2);
    let kept: Vec<String> = of_kind(&j, "t.full")
        .await
        .into_iter()
        .map(|e| e.message)
        .collect();
    assert_eq!(kept, vec!["0", "после"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_batch_retries_one_by_one() {
    // Ревью t158 Н2: запись, которую база не принимает (здесь — ограничение CHECK таблицы), не уносит с собой пачку.
    let d = std::env::temp_dir().join(format!("arb-j341-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let path = d.join("t.sqlite");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE journal (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, level TEXT NOT NULL,
             kind TEXT NOT NULL, venue TEXT, message TEXT NOT NULL, data TEXT, CHECK (message <> 'плохая'));",
        )
        .unwrap();
    }
    let (tx, _rx) = broadcast::channel(64);
    let j = Journal::new(Db::open(&path).unwrap(), tx);
    // Замок основной базы взят — три записи копятся и уходят пачкой.
    let hold = hold_main(&j, Duration::from_millis(300));
    for m in ["до", "плохая", "после"] {
        let _ = j.write(Level::Info, "t.check", None, m, None).await;
    }
    hold.await.unwrap();
    j.idle().await;
    let _ = j.write(Level::Info, "t.check", None, "ещё", None).await;
    let kept: Vec<String> = of_kind(&j, "t.check")
        .await
        .into_iter()
        .map(|e| e.message)
        .collect();
    assert_eq!(
        kept,
        vec!["до", "после", "ещё"],
        "теряется только не принятая запись"
    );
    let lost = of_kind(&j, LOST_KIND).await;
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(lost[0].data.as_ref().unwrap()["lost"], 1);
    let _ = std::fs::remove_dir_all(&d);
}
