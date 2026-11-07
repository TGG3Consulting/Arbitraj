//! Файл: tests/uncapped_by_deals.rs
//! Что проверяет: storage/uncapped_db.rs by_deals — последняя запись каждой сделки одним запросом (экран «Сделки»,
//! 06.11.2026: вместо запроса на каждую сделку); записи без сделки не попадают; совпадает с by_deal.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::storage::{db, uncapped_db}.

use arb::storage::db::Db;
use arb::storage::uncapped_db::{self, Row};

fn row(kind: &'static str, deal: Option<&str>, body: &str) -> Row {
    Row {
        kind,
        deal_id: deal.map(String::from),
        ticker: "CAKE".into(),
        address: None,
        started_at: "2026-11-06T10:00:00Z".into(),
        body: body.into(),
    }
}

#[tokio::test]
async fn latest_per_deal_in_one_query() {
    let d = std::env::temp_dir().join(format!("arb_uncapped_by_deals_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    uncapped_db::insert(&db, row("deal", Some("a"), "{\"n\":1}"))
        .await
        .unwrap();
    uncapped_db::insert(&db, row("candidate", None, "{\"n\":2}"))
        .await
        .unwrap();
    let a2 = uncapped_db::insert(&db, row("deal", Some("a"), "{\"n\":3}"))
        .await
        .unwrap();
    let b = uncapped_db::insert(&db, row("deal", Some("b"), "{\"n\":4}"))
        .await
        .unwrap();
    let mut got = uncapped_db::by_deals(&db).await.unwrap();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("a".into(), a2, "{\"n\":3}".into()),
            ("b".into(), b, "{\"n\":4}".into())
        ]
    );
    let one = uncapped_db::by_deal(&db, "a".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(one, (a2, "{\"n\":3}".into()));
}
