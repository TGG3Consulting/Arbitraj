//! Файл: tests/no_main_db_354.rs
//! Что делает: решение 354 (задача t158) — путь сделки и расчёта не ждёт основную базу: (1) правила шлюза читаются из
//! снимка в памяти (storage/rules.rs), снимок меняется сразу при создании и изменении правила, чтение не ждёт базу,
//! занятую долгой транзакцией; снимок есть сразу после открытия базы; (2) строки повторяющихся отказов (решение 155,
//! core/reject_log.rs) при включённой очереди журнала не ждут базу: метка в памяти, обновления и закрытие — по метке
//! через очередь, итог в базе тот же, что у прямого пути (одна строка на вид причины, счётчик, итог при закрытии).
//! Настоящая SQLite во временной папке, без моков.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::storage (db, journal, rules), arb::core (reject_log, reject_shape), arb::gateway::rules.

use std::time::{Duration, Instant};

use arb::core::reject_log::{Note, RejectLog};
use arb::core::reject_shape::shape;
use arb::gateway::errors::ErrorCode;
use arb::gateway::rules::{Rule, RuleAction, RuleScope};
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal, Level};
use tokio::sync::broadcast;

/// Временная папка теста.
fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("arb-n354-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Держит замок основной базы (долгая транзакция) заданное время; возвращается, когда замок взят.
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

/// Правило «повторять» на код.
fn retry(code: ErrorCode, attempts: u32) -> Rule {
    Rule {
        id: 0,
        event: code,
        condition: None,
        action: RuleAction::Retry {
            attempts,
            pause_ms: 1,
        },
        scope: RuleScope::All,
        enabled: true,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_from_memory_follow_changes() {
    let d = dir("rules");
    let p = d.join("t.sqlite");
    {
        let db = Db::open(&p).unwrap();
        assert!(arb::storage::rules::list(&db).await.unwrap().is_empty());
        let r = arb::storage::rules::create(&db, retry(ErrorCode::RateLimited, 2))
            .await
            .unwrap();
        let got = arb::storage::rules::list(&db).await.unwrap();
        assert_eq!(got.len(), 1, "создание видно сразу");
        let mut r2 = r.clone();
        r2.enabled = false;
        assert!(arb::storage::rules::update(&db, r2).await.unwrap());
        assert!(
            !arb::storage::rules::list(&db).await.unwrap()[0].enabled,
            "изменение видно сразу"
        );
    }
    // Новое открытие: снимок прочитан при открытии — чтение не ждёт базу, занятую долгой транзакцией.
    let db = Db::open(&p).unwrap();
    let hold = hold_main(&db, Duration::from_millis(800));
    let t0 = Instant::now();
    let got = arb::storage::rules::list(&db).await.unwrap();
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(300),
        "правила ждали базу: {took:?}"
    );
    assert_eq!(got.len(), 1);
    assert!(!got[0].enabled);
    hold.await.unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

/// Отказ кандидата в формате core/candidates.rs.
fn note(reason: &str) -> Note<'static> {
    Note {
        level: Level::Info,
        kind: "core.candidate_rejected",
        venue: Some("gate"),
        exact: reason.to_string(),
        shape: shape(reason),
        message: format!("USDC gate → bsc_dex: отказ — {reason}"),
        data: None,
    }
}

/// Строки вида core.candidate_rejected, старые первыми (после того, как очередь опустела).
async fn rows(j: &Journal) -> Vec<Entry> {
    j.idle().await;
    let mut v: Vec<Entry> = j.list(100, None).await.unwrap();
    v.retain(|e| e.kind == "core.candidate_rejected");
    v.reverse();
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_repeat_rows_do_not_wait_and_match_direct() {
    let d = dir("reject");
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(1024);
    let j = Journal::new(db.clone(), tx);
    j.set_queue_max(Some(10_000));
    let log = RejectLog::default();
    let k = Some(Duration::ZERO);
    let hold = hold_main(&db, Duration::from_millis(800));
    let t0 = Instant::now();
    for p in ["-0.5184", "-0.5191", "-0.5177", "-0.5203", "-0.5168"] {
        let r = format!("прибыль {p} USDT ниже порога 0.1");
        log.note(&j, "d", note(&r), k).await;
    }
    log.note(&j, "d", note("нет данных стакана gate"), k).await;
    log.note(&j, "d", note("нет данных стакана gate"), k).await;
    log.close(&j, "d").await;
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(300),
        "отказы ждали базу: {took:?}"
    );
    hold.await.unwrap();
    let v = rows(&j).await;
    assert_eq!(v.len(), 2, "{v:?}");
    assert_eq!(v[0].repeats, Some(5), "первый вид — итог 5");
    assert!(v[0].message.contains("-0.5168"), "текст — последнего");
    assert_eq!(v[1].repeats, Some(2), "второй вид — итог при закрытии");
    let _ = std::fs::remove_dir_all(&d);
}
