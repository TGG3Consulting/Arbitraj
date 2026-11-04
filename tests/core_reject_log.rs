//! Файл: tests/core_reject_log.rs
//! Что делает: решение 155 — журнал отказов кандидатов не засыпается одной и той же причиной. Живой журнал 04.11:
//! ~11 записей core.candidate_rejected в секунду («USDC gate → bsc_dex: отказ — прибыль -0.5184 USDT ниже порога …»),
//! числа прибыли меняются на каждом расчёте. Проверяется на настоящем хранилище (SQLite во временной папке):
//! 5 отказов одного вида причины с разными числами → одна строка со счётчиком 5, «по» — время последнего, текст —
//! последний; смена вида причины → вторая строка, первая закрыта итогом; редкое обновление (journal.repeat_flush_ms
//! больше паузы) — итог дописывается при закрытии; настройка не задана — прежний режим (строка на каждый отказ).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::reject_log, arb::core::reject_shape, arb::storage (db, journal). Моков нет.

use std::time::Duration;

use arb::core::reject_log::{Note, RejectLog};
use arb::core::reject_shape::shape;
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal, Level};
use tokio::sync::broadcast;

/// Журнал на базе во временной папке.
fn journal(name: &str) -> Journal {
    let d = std::env::temp_dir().join(format!("arb-reject-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(64);
    Journal::new(db, tx)
}

/// Отказ в формате core/candidates.rs + core/profit.rs (живой текст 04.11).
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

/// Записи вида core.candidate_rejected, старые первыми.
async fn rows(j: &Journal) -> Vec<Entry> {
    let mut v: Vec<Entry> = j.list(100, None).await.unwrap();
    v.retain(|e| e.kind == "core.candidate_rejected");
    v.reverse();
    v
}

const PROFITS: [&str; 5] = ["-0.5184", "-0.5191", "-0.5177", "-0.5203", "-0.5168"];

#[test]
fn shape_drops_numbers_only() {
    assert_eq!(
        shape("прибыль -0.5184 USDT ниже порога 0.1 (объём 120,5 USDC)"),
        "прибыль # USDT ниже порога # (объём # USDC)"
    );
    assert_eq!(shape("прибыль −0.5184"), shape("прибыль 3"));
    assert_ne!(
        shape("прибыль -1 USDT ниже порога 0.1"),
        shape("прибыль -1% ниже порога 0.1%")
    );
}

#[tokio::test]
async fn five_same_one_row_then_change_second_row() {
    let j = journal("five");
    let log = RejectLog::default();
    let k = Some(Duration::ZERO); // 0 — строка обновляется на каждый повтор.
    for p in PROFITS {
        let r = format!("прибыль {p} USDT ниже порога 0.1 (лучший объём по стакану)");
        log.note(&j, "USDC gate→bsc_dex", note(&r), k).await;
    }
    let v = rows(&j).await;
    assert_eq!(v.len(), 1, "5 одинаковых по виду отказов — одна строка");
    assert_eq!(v[0].repeats, Some(5));
    assert!(
        v[0].message.contains("-0.5168"),
        "текст — последнего расчёта"
    );
    assert!(v[0].last_at.unwrap() >= v[0].at);

    // Смена вида причины — вторая строка, первая остаётся с итогом 5.
    log.note(&j, "USDC gate→bsc_dex", note("нет данных стакана gate"), k)
        .await;
    let v = rows(&j).await;
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].repeats, Some(5));
    assert_eq!(v[1].repeats, Some(1));

    // Другое направление — своя строка.
    log.note(&j, "USDC bsc_dex→gate", note("нет данных стакана gate"), k)
        .await;
    assert_eq!(rows(&j).await.len(), 3);
}

#[tokio::test]
async fn rare_flush_total_written_on_close() {
    let j = journal("close");
    let log = RejectLog::default();
    let k = Some(Duration::from_secs(3600)); // реже, чем идут повторы
    for p in PROFITS {
        let r = format!("прибыль {p} USDT ниже порога 0.1");
        log.note(&j, "d", note(&r), k).await;
    }
    assert_eq!(
        rows(&j).await[0].repeats,
        Some(1),
        "до срока строка в базе не обновляется"
    );
    log.close(&j, "d").await; // кандидат прошёл
    let v = rows(&j).await;
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].repeats, Some(5), "итог дописан при закрытии");
    // После прохождения тот же отказ — новая строка.
    log.note(&j, "d", note("прибыль -0.5 USDT ниже порога 0.1"), k)
        .await;
    assert_eq!(rows(&j).await.len(), 2);
}

#[tokio::test]
async fn delayed_flush_writes_missed_repeats() {
    let j = journal("delayed");
    let log = RejectLog::default();
    let k = Some(Duration::from_millis(200));
    for p in PROFITS {
        log.note(
            &j,
            "d",
            note(&format!("прибыль {p} USDT ниже порога 0.1")),
            k,
        )
        .await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        rows(&j).await[0].repeats,
        Some(5),
        "повторы прекратились — итог дописан отложенно"
    );
}

#[tokio::test]
async fn not_set_writes_each_as_before() {
    let j = journal("plain");
    let log = RejectLog::default();
    for p in PROFITS {
        log.note(
            &j,
            "d",
            note(&format!("прибыль {p} USDT ниже порога 0.1")),
            None,
        )
        .await;
    }
    let v = rows(&j).await;
    assert_eq!(
        v.len(),
        5,
        "настройка не задана — каждый отказ с другим текстом отдельной строкой"
    );
    assert!(v.iter().all(|e| e.repeats.is_none()));
}

#[tokio::test]
async fn old_base_gets_columns() {
    // База до решения 155: таблица journal без repeats и last_at, в ней уже есть запись.
    let d = std::env::temp_dir().join(format!("arb-reject-old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let path = d.join("t.sqlite");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE journal (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, level TEXT NOT NULL,
             kind TEXT NOT NULL, venue TEXT, message TEXT NOT NULL, data TEXT);
             INSERT INTO journal (at, level, kind, message) VALUES ('2026-11-04T10:00:00+00:00', 'Info', 'x', 'старая');",
        )
        .unwrap();
    }
    let (tx, _) = broadcast::channel(4);
    let j = Journal::new(Db::open(&path).unwrap(), tx);
    let v = j.list(10, None).await.unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].repeats, None);
    RejectLog::default()
        .note(&j, "d", note("нет данных"), Some(Duration::ZERO))
        .await;
    assert_eq!(rows(&j).await[0].repeats, Some(1));
}
