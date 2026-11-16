//! Файл: tests/store_mark_355.rs
//! Что делает: решение 355 (ревью t158 Б1) — запрещающая отметка ядра (Store::mark_now, settings/store_mark.rs)
//! действует сразу: значение видно в настройках, пока основная база занята долгой транзакцией (как пачкой журнала), —
//! окна нет; в базу оно ложится своей задачей (после перезапуска хранилища значение на месте) вместе с историей;
//! список считается из текущего (два значения подряд не теряют друг друга); изменение человеком после отметки —
//! сильнее (фоновая запись не перетирает его устаревшим). Настоящая SQLite во временной папке, без моков.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings (store, store_mark, value), arb::storage (db, journal).

use std::time::{Duration, Instant};

use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Хранилище на базе в папке и сама база (её замок держит тест).
async fn store(d: &std::path::Path) -> (Store, Db) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let s = Store::load(db.clone(), changes, Journal::new(db.clone(), jtx))
        .await
        .unwrap();
    (s, db)
}

/// Временная папка.
fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("arb-m355-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Держит замок основной базы заданное время; возвращается, когда замок взят.
fn hold_main(db: &Db, for_: Duration) -> tokio::task::JoinHandle<()> {
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let db = db.clone();
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

/// Ждёт, пока значение в базе (новое хранилище на той же папке) станет нужным; нет за 5 с — тест падает.
async fn persisted(d: &std::path::Path, name: &str, scope: &Scope, want: Option<Value>) {
    let t0 = Instant::now();
    loop {
        let (s, _) = store(d).await;
        if s.get_exact(name, scope) == want {
            return;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "в базу не легло: {name}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mark_has_no_window_and_persists() {
    let d = dir("window");
    let (s, db) = store(&d).await;
    let coin = Scope::Coin("CAKE".into());
    let venues = Scope::Coin("CAKE".into());
    let hold = hold_main(&db, Duration::from_millis(800));
    // База занята — отметка всё равно видна сразу (читатели — кэш настроек), шаг не ждёт базу.
    let t0 = Instant::now();
    s.mark_now("coin.blocked", coin.clone(), |_| Some(json!(true)), "тест")
        .unwrap();
    assert_eq!(
        s.get_exact("coin.blocked", &coin),
        Some(json!(true)),
        "окна нет"
    );
    // Список — из текущего: две биржи подряд не теряют друг друга.
    for v in ["binance", "mexc"] {
        let add = |cur: Option<&Value>| {
            let mut l: Vec<Value> = cur.and_then(Value::as_array).cloned().unwrap_or_default();
            l.push(json!(v));
            Some(Value::Array(l))
        };
        s.mark_now("coin.entry_blocked_venues", venues.clone(), add, "тест")
            .unwrap();
    }
    assert!(
        t0.elapsed() < Duration::from_millis(300),
        "отметка ждала базу: {:?}",
        t0.elapsed()
    );
    assert_eq!(
        s.get_exact("coin.entry_blocked_venues", &venues),
        Some(json!(["binance", "mexc"]))
    );
    hold.await.unwrap();
    persisted(&d, "coin.blocked", &coin, Some(json!(true))).await;
    persisted(
        &d,
        "coin.entry_blocked_venues",
        &venues,
        Some(json!(["binance", "mexc"])),
    )
    .await;
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test(flavor = "multi_thread")]
async fn later_human_change_wins() {
    let d = dir("human");
    let (s, db) = store(&d).await;
    let coin = Scope::Coin("PIG".into());
    let hold = hold_main(&db, Duration::from_millis(400));
    s.mark_now("coin.blocked", coin.clone(), |_| Some(json!(true)), "тест")
        .unwrap();
    // Человек снимает отметку, пока фоновая запись отметки ждёт базу: его значение — последнее.
    let human = {
        let s = s.clone();
        let coin = coin.clone();
        tokio::spawn(async move {
            s.set("coin.blocked", coin, Some(json!(false)), "человек")
                .await
        })
    };
    hold.await.unwrap();
    human.await.unwrap().unwrap();
    assert_eq!(s.get_exact("coin.blocked", &coin), Some(json!(false)));
    persisted(&d, "coin.blocked", &coin, Some(json!(false))).await;
    let _ = std::fs::remove_dir_all(&d);
}
