//! Файл: tests/uncapped_missed_only.rs
//! Что проверяет: решение 197 — выборка окна «Упущено из-за потолка» с missed_only (storage/uncapped_db.rs Filter):
//! сделки — все (и с упущено 0: «потолок не мешал» тоже знание), кандидаты — только с упущено > 0 (missed — десятичная
//! строка JSON записи); без missed_only — все записи (старые записи не удаляются).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::storage::{db, uncapped_db}.

use arb::storage::db::Db;
use arb::storage::uncapped_db::{self, Filter, Row};

fn row(kind: &'static str, deal: Option<&str>, missed: &str) -> Row {
    Row {
        kind,
        deal_id: deal.map(String::from),
        ticker: "CAKE".into(),
        address: None,
        started_at: "2026-11-06T10:00:00.000Z".into(),
        body: format!("{{\"missed\":\"{missed}\"}}"),
    }
}

#[tokio::test]
async fn deals_all_candidates_only_with_missed() {
    let d = std::env::temp_dir().join(format!("arb_uncapped_missed_only_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let deal0 = uncapped_db::insert(&db, row("deal", Some("a"), "0"))
        .await
        .unwrap();
    let deal1 = uncapped_db::insert(&db, row("deal", Some("b"), "1.5"))
        .await
        .unwrap();
    let _c0 = uncapped_db::insert(&db, row("candidate", None, "0"))
        .await
        .unwrap();
    let c1 = uncapped_db::insert(&db, row("candidate", None, "0.0001"))
        .await
        .unwrap();
    let f = Filter {
        missed_only: true,
        ..Filter::default()
    };
    let mut ids: Vec<i64> = uncapped_db::all(&db, f)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.0)
        .collect();
    ids.sort();
    assert_eq!(ids, vec![deal0, deal1, c1]);
    let every = uncapped_db::all(&db, Filter::default()).await.unwrap();
    assert_eq!(every.len(), 4, "без missed_only — все записи");
}
