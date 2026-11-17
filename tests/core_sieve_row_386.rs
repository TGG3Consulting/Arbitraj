//! Файл: tests/core_sieve_row_386.rs
//! Что делает: решение 386 — строка отсева кандидата «при изменении», без таймера (sieve.row_interval_ms удалена).
//! Показ (строка «Кандидатов» в браузер, закрытие эпизода — core/sieve_log.rs cut) идёт только тогда, когда пишется
//! запись журнала core.candidate_sieved механизмом повторов (core/reject_log.rs): первый отсев направления и смена ВИДА
//! причины (числа → «#») — новая строка и показ; повтор того же вида — счётчик на старой строке, показ — только когда
//! строка повторов обновляется сейчас (journal.repeat_flush_ms); journal.repeat_flush_ms не задана — показ только при
//! смене вида. RejectLog::due (по нему готовятся данные показа, ревью t171 Н9) совпадает с итогом note. Проверяется
//! на настоящем хранилище (SQLite во временной папке) с текстами причины в формате core/sieve.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{reject_log, reject_shape, sieve_log}, arb::storage (db, journal). Моков нет.

use std::time::Duration;

use arb::core::reject_log::{Note, Noted, RejectLog};
use arb::core::reject_shape::shape;
use arb::core::sieve_log::show;
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal, Level};
use tokio::sync::broadcast;

/// Журнал на базе во временной папке.
fn journal(name: &str) -> Journal {
    let d = std::env::temp_dir().join(format!("arb-sieve-row-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(64);
    Journal::new(db, tx)
}

/// Отсев по USDT в формате core/sieve.rs (числа — переменные).
fn by_usd(bound: &str) -> String {
    format!(
        "отсев по границе: прибыль не больше {bound} USDT (на 120 CAKE) < порога 0.1 (стакан bingx, верх DEX)"
    )
}

/// Отсев по % в формате core/sieve.rs — другой вид причины.
fn by_pct(gap: &str) -> String {
    format!(
        "отсев по границе: прибыль ниже порога 0.2% на любом объёме (запас до порога {gap} USDT < 0; стакан bingx)"
    )
}

/// Запись отсева, как в core/sieve_log.rs cut (exact = вид причины).
fn note(reason: &str) -> Note<'static> {
    Note {
        level: Level::Info,
        kind: "core.candidate_sieved",
        venue: Some("bingx"),
        exact: shape(reason),
        shape: shape(reason),
        message: format!("CAKE bingx → bsc_dex: {reason}"),
        data: None,
    }
}

/// Записи core.candidate_sieved, старые первыми.
async fn rows(j: &Journal) -> Vec<Entry> {
    let mut v: Vec<Entry> = j.list(100, None).await.unwrap();
    v.retain(|e| e.kind == "core.candidate_sieved");
    v.reverse();
    v
}

#[test]
fn show_only_when_written() {
    assert!(show(Noted::New));
    assert!(show(Noted::Flushed));
    assert!(!show(Noted::Held));
}

#[tokio::test]
async fn shown_on_first_and_on_kind_change_only() {
    let j = journal("rare");
    let log = RejectLog::default();
    // Строка повторов обновляется редко — повторы того же вида не показываются.
    let k = Some(Duration::from_secs(3600));
    let first = log.note(&j, "d", note(&by_usd("-0.51")), k).await;
    assert_eq!(first, Noted::New, "первый отсев — новая строка и показ");
    for b in ["-0.52", "-0.49", "-0.50"] {
        let n = log.note(&j, "d", note(&by_usd(b)), k).await;
        assert!(!show(n), "тот же вид причины, другие числа — без показа");
    }
    let changed = log.note(&j, "d", note(&by_pct("-0.03")), k).await;
    assert_eq!(
        changed,
        Noted::New,
        "смена вида причины — новая строка и показ"
    );
    let v = rows(&j).await;
    assert_eq!(v.len(), 2, "две строки — по одной на вид причины");
    assert_eq!(
        v[0].repeats,
        Some(4),
        "повторы — счётчиком на старой строке"
    );
}

#[tokio::test]
async fn repeat_shown_when_row_flushed() {
    let j = journal("flush");
    let log = RejectLog::default();
    let k = Some(Duration::ZERO);
    assert_eq!(
        log.note(&j, "d", note(&by_usd("-0.51")), k).await,
        Noted::New
    );
    let n = log.note(&j, "d", note(&by_usd("-0.52")), k).await;
    assert_eq!(
        n,
        Noted::Flushed,
        "строка повторов обновлена сейчас — числа показываются"
    );
    assert!(show(n));
}

#[tokio::test]
async fn plain_mode_shows_on_kind_change() {
    let j = journal("plain");
    let log = RejectLog::default();
    assert_eq!(
        log.note(&j, "d", note(&by_usd("-0.51")), None).await,
        Noted::New
    );
    assert_eq!(
        log.note(&j, "d", note(&by_usd("-0.52")), None).await,
        Noted::Held
    );
    assert_eq!(
        log.note(&j, "d", note(&by_pct("-0.03")), None).await,
        Noted::New
    );
    assert_eq!(rows(&j).await.len(), 2);
}

#[tokio::test]
async fn full_calc_closes_row_next_sieve_shown() {
    let j = journal("close");
    let log = RejectLog::default();
    let k = Some(Duration::from_secs(3600));
    assert_eq!(
        log.note(&j, "d", note(&by_usd("-0.51")), k).await,
        Noted::New
    );
    // Кандидат прошёл полный расчёт (candidates.rs accepted_timed) — строка по направлению закрыта.
    log.close(&j, "d").await;
    assert_eq!(
        log.note(&j, "d", note(&by_usd("-0.52")), k).await,
        Noted::New,
        "после расчёта отсев того же вида снова показывается сразу"
    );
}

#[tokio::test]
async fn due_predicts_note() {
    // Ревью t171, Н9: данные показа готовятся по due — он должен совпадать с итогом note.
    for (name, k) in [
        ("due-rare", Some(Duration::from_secs(3600))),
        ("due-zero", Some(Duration::ZERO)),
        ("due-plain", None),
    ] {
        let j = journal(name);
        let log = RejectLog::default();
        let seq = [
            by_usd("-0.51"),
            by_usd("-0.52"),
            by_usd("-0.50"),
            by_pct("-0.03"),
            by_pct("-0.04"),
            by_usd("-0.49"),
        ];
        for r in seq {
            let s = shape(&r);
            let want = log.due("d", &s, &s, k);
            let got = log.note(&j, "d", note(&r), k).await;
            assert_eq!(want, show(got), "{name}: due = показ для «{r}» ({got:?})");
        }
    }
}
