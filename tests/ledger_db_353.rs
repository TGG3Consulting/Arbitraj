//! Файл: tests/ledger_db_353.rs
//! Что делает: решение 353 (задача t158) — учёт ядра в своём файле ledger.sqlite (storage/ledger_db.rs): запись учёта не
//! ждёт замка основной базы (основная база занята долгой транзакцией — запись сделки проходит сразу); перенос из
//! основной базы при открытии (нет в файле учёта — копируется, в основной новее — заменяет, в файле учёта новее —
//! остаётся; основная база не меняется); PRAGMA synchronous писателя — по настройке и обратно к значению при открытии.
//! Настоящая SQLite во временной папке, без моков.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::storage::db, arb::storage::ledger_db, arb::core::persist.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use arb::storage::db::Db;
use rusqlite::params;

/// Чистая временная папка теста.
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-l353-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Строка core_state в ОСНОВНОЙ базе (как писала прежняя версия бота). Ошибок нет (тест падает).
async fn put_main(db: &Db, id: &str, body: &str, at: &str) {
    let (id, body, at) = (id.to_string(), body.to_string(), at.to_string());
    db.call(move |c| {
        c.execute(
            "INSERT INTO core_state (kind, id, body, updated_at) VALUES ('deal', ?1, ?2, ?3)
             ON CONFLICT(kind, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![id, body, at],
        )
        .map(|_| ())
    })
    .await
    .unwrap();
}

/// Строка core_state в ФАЙЛЕ УЧЁТА. Ошибок нет (тест падает).
async fn put_ledger(db: &Db, id: &str, body: &str, at: &str) {
    let (id, body, at) = (id.to_string(), body.to_string(), at.to_string());
    db.ledger()
        .write(move |c| {
            c.execute(
                "INSERT INTO core_state (kind, id, body, updated_at) VALUES ('deal', ?1, ?2, ?3)
                 ON CONFLICT(kind, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                params![id, body, at],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
}

/// Тело сделки id из файла учёта (None — нет строки).
async fn ledger_body(db: &Db, id: &str) -> Option<String> {
    let id = id.to_string();
    db.ledger()
        .read(move |c| {
            let mut st =
                c.prepare("SELECT body FROM core_state WHERE kind = 'deal' AND id = ?1")?;
            let mut it = st.query_map(params![id], |r| r.get::<_, String>(0))?;
            it.next().transpose()
        })
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn ledger_write_does_not_wait_main_db() {
    let d = dir("nowait");
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    // Основная база занята долгой транзакцией (как длинная пачка журнала): держим её замок 1 с.
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let busy = db.clone();
    let hold = tokio::task::spawn_blocking(move || {
        busy.call_sync(|c| {
            let t = c.transaction()?;
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_secs(1));
            t.commit()
        })
    });
    held_rx.recv().unwrap();
    let t0 = Instant::now();
    arb::core::persist::save(&db, "deal", "D1", &serde_json::json!({ "step": "Buying" }))
        .await
        .unwrap();
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(500),
        "запись сделки ждала основную базу: {took:?}"
    );
    let rows: Vec<serde_json::Value> = arb::core::persist::load(&db, "deal").await.unwrap();
    assert_eq!(rows, vec![serde_json::json!({ "step": "Buying" })]);
    hold.await.unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn move_from_main_keeps_newest() {
    let d = dir("move");
    let p = d.join("t.sqlite");
    {
        let db = Db::open(&p).unwrap();
        assert_eq!(db.ledger().moved(), 0, "пустая база — переносить нечего");
        // Старая версия писала в основную базу.
        put_main(&db, "only_main", "m1", "2026-11-16T10:00:00+00:00").await;
        put_main(&db, "main_newer", "m2", "2026-11-16T12:00:00.5+00:00").await;
        put_main(&db, "led_newer", "m3", "2026-11-16T10:00:00+00:00").await;
        put_main(&db, "bad_time", "m4", "t").await;
        put_ledger(&db, "main_newer", "l2", "2026-11-16T12:00:00.25+00:00").await;
        put_ledger(&db, "led_newer", "l3", "2026-11-16T11:00:00+00:00").await;
        put_ledger(&db, "bad_time", "l4", "2026-11-16T11:00:00+00:00").await;
    }
    let db = Db::open(&p).unwrap();
    assert_eq!(db.ledger().moved(), 2, "only_main и main_newer");
    assert_eq!(ledger_body(&db, "only_main").await.as_deref(), Some("m1"));
    assert_eq!(ledger_body(&db, "main_newer").await.as_deref(), Some("m2"));
    assert_eq!(ledger_body(&db, "led_newer").await.as_deref(), Some("l3"));
    assert_eq!(ledger_body(&db, "bad_time").await.as_deref(), Some("l4"));
    // Основная база не тронута (откат на старую версию видит прежние строки).
    let main: Vec<String> = db
        .call(|c| {
            let mut st = c.prepare("SELECT body FROM core_state ORDER BY id")?;
            let it = st.query_map([], |r| r.get(0))?;
            it.collect()
        })
        .await
        .unwrap();
    assert_eq!(main, vec!["m4", "m3", "m2", "m1"]);
    drop(db);
    // Третье открытие — переносить нечего.
    let db = Db::open(&p).unwrap();
    assert_eq!(db.ledger().moved(), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn synchronous_follows_setting() {
    let d = dir("sync");
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let now = || {
        db.ledger()
            .write(|c| c.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0)))
    };
    let at_open = now().await.unwrap();
    // https://www.sqlite.org/pragma.html#pragma_synchronous: 0 OFF, 1 NORMAL, 2 FULL, 3 EXTRA.
    db.ledger().set_synchronous(Some("NORMAL".into()));
    assert_eq!(now().await.unwrap(), 1);
    db.ledger().set_synchronous(Some("EXTRA".into()));
    assert_eq!(now().await.unwrap(), 3);
    db.ledger().set_synchronous(None);
    assert_eq!(
        now().await.unwrap(),
        at_open,
        "снятие настройки — значение при открытии"
    );
    let _ = std::fs::remove_dir_all(&d);
}
