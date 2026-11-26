//! Граница UTC-окна 08:51 и рестарт внутри минуты. Записи проходят штатную очередь и SQLite.
use std::collections::BTreeMap;
use std::sync::mpsc;

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use super::{Ev, St};
use crate::storage::db::Db;
use crate::storage::journal::{Entry, Journal, Level};

fn journal(name: &str) -> (Journal, broadcast::Receiver<Entry>) {
    let d = std::env::temp_dir().join(format!(
        "arb-mexc-fast-{name}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&d).unwrap();
    let (tx, rx) = broadcast::channel(16);
    (
        Journal::new(Db::open(&d.join("journal.sqlite")).unwrap(), tx),
        rx,
    )
}

fn at(t: &str) -> DateTime<Utc> {
    format!("2026-11-26T{t}Z").parse().unwrap()
}

async fn minutes(j: &Journal) -> Vec<Entry> {
    j.list(30, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| {
            e.kind == "venue.fast_book"
                && e.data
                    .as_ref()
                    .is_some_and(|d| d["minute_counts"].is_object())
        })
        .collect()
}

#[tokio::test]
async fn two_sessions_and_a_boundary_have_additive_utc_minutes() {
    let (j, _) = journal("boundary");
    // Старая накопительная строка остаётся читаемой, в сумму по минутам не входит.
    j.write(
        Level::Warn,
        "venue.fast_book",
        Some("mexc"),
        "старый формат",
        Some(serde_json::json!({"resyncs":47526})),
    )
    .await
    .unwrap();
    let mut session = St::default();
    session.note(&j, at("08:50:58"), Ev::ToFast, "USDC", "кадр");
    session.note(&j, at("08:50:59"), Ev::Resync, "USDC", "запрос");
    session.note(&j, at("08:51:00"), Ev::SnapFail, "TST", "отказ REST");
    j.idle().await;
    let mut next_session = St::default();
    next_session.note(&j, at("08:51:50"), Ev::Resync, "USDC", "разрыв");
    next_session.note(&j, at("08:51:51"), Ev::Resync, "USDC", "разрыв");
    let rows = minutes(&j).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.iter().map(|e| e.repeats.unwrap()).sum::<i64>(), 5);
    let mut sums = BTreeMap::<i64, (u64, u64)>::new();
    for row in rows {
        let d = row.data.unwrap();
        let m = sums
            .entry(d["minute_start_utc_s"].as_i64().unwrap())
            .or_default();
        m.0 += d["minute_counts"]["resyncs"].as_u64().unwrap();
        m.1 += d["minute_counts"]["snapshot_failed"].as_u64().unwrap();
    }
    assert_eq!(sums[&at("08:50:00").timestamp()], (1, 0));
    assert_eq!(sums[&at("08:51:00").timestamp()], (2, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_first_insert_preserves_its_minute_on_retry() {
    let (j, mut live) = journal("full");
    j.set_queue_max(Some(1));
    let (held, acquired) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let db = j.db().clone();
    let worker = tokio::task::spawn_blocking(move || {
        db.call_sync(|_| {
            held.send(()).unwrap();
            wait.recv().unwrap();
            Ok(())
        })
        .unwrap()
    });
    acquired.recv().unwrap();
    j.write(Level::Info, "control", None, "заняли очередь", None)
        .await
        .unwrap();

    let mut state = St::default();
    state.note(&j, at("08:50:58"), Ev::Resync, "USDC", "разрыв");
    state.note(&j, at("08:50:59"), Ev::Resync, "USDC", "разрыв");
    state.note(&j, at("08:51:00"), Ev::SnapFail, "USDC", "отказ");
    assert!(state.token.is_none());
    assert_eq!(state.unsaved.len(), 1);
    release.send(()).unwrap();
    worker.await.unwrap();
    j.idle().await;
    j.set_queue_max(None);
    state.note(&j, at("08:51:20"), Ev::ToSnap, "USDC", "резервная книга");

    let rows = minutes(&j).await;
    assert_eq!(rows.len(), 2);
    let mut published = BTreeMap::new();
    while let Ok(e) = live.try_recv() {
        if e.kind == "venue.fast_book" {
            published.insert(e.id, e.repeats);
        }
    }
    for r in rows {
        let data = r.data.unwrap();
        let old = data["minute_start_utc_s"].as_i64().unwrap() == at("08:50:00").timestamp();
        assert_eq!(data["minute_counts"]["resyncs"], if old { 2 } else { 0 });
        assert_eq!(
            data["minute_counts"]["snapshot_failed"],
            if old { 0 } else { 1 }
        );
        assert_eq!(r.repeats, Some(2));
        assert_eq!(published.get(&r.id), Some(&r.repeats));
    }
}
