//! Файл: tests/settings_snapshot_409.rs
//! Что делает: решение 409 (t194) — значения настроек в памяти как снимок за ArcSwap: несколько потоков читают
//! настройки без замка, пока один пишет. Читатель видит значение целиком (старое или новое, никогда «нет значения»),
//! значения у одного читателя не идут назад (снимки публикуются по порядку), набор ключей (all) не рвётся; копия на
//! момент входа (pinned, решение 239) не меняется от последующих записей; отметка ядра (mark_now) и set видны сразу.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings, arb::storage. Моков нет — настоящая база SQLite во временной папке.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arb::settings::read::At;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use serde_json::json;
use tokio::sync::broadcast;

/// Новая база во временной папке. Возвращает хранилище.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(1024);
    let (jtx, _) = broadcast::channel(1024);
    let journal = arb::storage::journal::Journal::new(db.clone(), jtx);
    Store::load(db, tx, journal).await.unwrap()
}

const NAME: &str = "signal.min_hold_updates";
const OTHER: &str = "feed.stale_after_ms";
const WRITES: u64 = 60;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readers_see_whole_snapshots_while_writer_writes() {
    let s = fresh("snap409").await;
    s.set(NAME, Scope::Global, Some(json!(1)), "т")
        .await
        .unwrap();
    s.set(OTHER, Scope::Venue("bingx".into()), Some(json!(1500)), "т")
        .await
        .unwrap();
    let keys_before = s.all().len();
    let pinned = s.pinned();
    let stop = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();
    for _ in 0..4 {
        let (s, stop) = (s.clone(), stop.clone());
        readers.push(std::thread::spawn(move || {
            let mut last = 0u64;
            let mut reads = 0u64;
            while !stop.load(Ordering::Acquire) {
                let v = s
                    .integer(NAME, At::default())
                    .expect("значение есть всегда");
                assert!(v >= last, "значение пошло назад: {v} после {last}");
                assert!((1..=WRITES).contains(&v));
                last = v;
                let at = At {
                    venue: Some("bingx"),
                    coin: None,
                };
                assert_eq!(s.integer(OTHER, at).unwrap(), 1500);
                assert_eq!(s.all().len(), keys_before, "набор ключей не рвётся");
                reads += 1;
            }
            reads
        }));
    }
    for i in 2..=WRITES {
        s.set(NAME, Scope::Global, Some(json!(i)), "т")
            .await
            .unwrap();
        // Запись видна сразу после возврата set.
        assert_eq!(s.integer(NAME, At::default()).unwrap(), i);
    }
    stop.store(true, Ordering::Release);
    for r in readers {
        assert!(r.join().expect("читатель без паники") > 0);
    }
    // Копия на момент входа (решение 239) — прежнее значение, хотя живое хранилище изменилось.
    assert_eq!(pinned.integer(NAME, At::default()).unwrap(), 1);
    assert_eq!(s.integer(NAME, At::default()).unwrap(), WRITES);
    // Поколение (решение 352) выросло на каждую запись.
    assert!(s.generation() >= WRITES + 1);
}

#[tokio::test]
async fn mark_now_visible_at_once_and_pinned_unchanged() {
    let s = fresh("snap409mark").await;
    let pinned = s.pinned();
    let gen0 = s.generation();
    let coin = Scope::Coin("CAKE".into());
    let ch = s
        .mark_now("coin.blocked", coin.clone(), |_| Some(json!(true)), "т")
        .unwrap();
    assert!(ch.is_some());
    assert_eq!(s.get_exact("coin.blocked", &coin), Some(json!(true)));
    assert_eq!(s.generation(), gen0 + 1);
    // То же значение ещё раз — изменения нет, поколение прежнее.
    let again = s
        .mark_now("coin.blocked", coin.clone(), |_| Some(json!(true)), "т")
        .unwrap();
    assert!(again.is_none());
    assert_eq!(s.generation(), gen0 + 1);
    assert_eq!(pinned.get_exact("coin.blocked", &coin), None);
}

#[tokio::test]
async fn clear_removes_from_all_and_lookup() {
    // Решение 409 (п. д): значения по имени и области — поиск без выделения; очистка убирает значение из all().
    let s = fresh("snap409clear").await;
    let bingx = Scope::Venue("bingx".into());
    let mexc = Scope::Venue("mexc".into());
    s.set(OTHER, bingx.clone(), Some(json!(1500)), "т")
        .await
        .unwrap();
    s.set(OTHER, mexc.clone(), Some(json!(900)), "т")
        .await
        .unwrap();
    assert_eq!(s.get_exact(OTHER, &bingx), Some(json!(1500)));
    assert_eq!(s.get_exact(OTHER, &mexc), Some(json!(900)));
    assert_eq!(s.get_exact(OTHER, &Scope::Global), None);
    let n = s.all().len();
    s.set(OTHER, bingx.clone(), None, "т").await.unwrap();
    assert_eq!(s.get_exact(OTHER, &bingx), None);
    assert_eq!(s.get_exact(OTHER, &mexc), Some(json!(900)));
    assert_eq!(s.all().len(), n - 1);
    s.set(OTHER, mexc.clone(), None, "т").await.unwrap();
    assert!(s.all().iter().all(|(name, _, _)| name != OTHER));
}
