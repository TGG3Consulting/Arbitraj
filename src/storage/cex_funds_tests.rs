//! NET-FUNDS. Две конкурентные заявки, сверка проводки и повторное открытие того же ledger.sqlite.
use super::*;
use crate::storage::db::Db;
use std::path::PathBuf;

fn file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arb-funds-{name}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir(&dir).unwrap();
    dir.join("state.sqlite")
}
fn bucket() -> Bucket {
    Bucket {
        venue: "spot".into(),
        account: "original".into(),
        currency: "USDT".into(),
    }
}
fn money(q: i64) -> Decimal {
    Decimal::from(q)
}
fn claim(id: &str, n: i64, funding: bool) -> Claim {
    Claim {
        action: id.into(),
        deal: id.into(),
        client: format!("client-{id}"),
        cap: money(n),
        funding,
    }
}
fn snapshot(b: &Bucket, epoch: i64, free: i64) -> Balance {
    Balance {
        bucket: b.clone(),
        epoch,
        free: money(free),
        booked: vec![],
    }
}

#[tokio::test]
async fn concurrent_normal_and_mixed_100_cannot_both_send_80_and_restart_keeps_original_id() {
    let path = file("race");
    let b = bucket();
    let db = Db::open(&path).unwrap();
    let l = db.ledger();
    let (a, z) = tokio::join!(
        admit(
            l,
            &b,
            claim("normal", 80, false),
            snapshot(&b, 0, 100),
            Holds::default()
        ),
        admit(
            l,
            &b,
            claim("mixed", 80, true),
            snapshot(&b, 0, 100),
            Holds::default()
        )
    );
    let (first, mixed, next) = match (a.unwrap(), z.unwrap()) {
        (Admission::Send, Admission::Recheck) => ("normal", false, "mixed"),
        (Admission::Recheck, Admission::Send) => ("mixed", true, "normal"),
        other => panic!("две заявки на один остаток: {other:?}"),
    };
    drop(db);
    let db = Db::open(&path).unwrap();
    let l = db.ledger();
    let rev = epoch(l, &b).await.unwrap();
    let mut retry = claim(first, 80, mixed);
    retry.client = "новый ID, который адаптер успел подготовить при повторе".into();
    assert_eq!(
        admit(l, &b, retry, snapshot(&b, rev, 100), Holds::default())
            .await
            .unwrap(),
        Admission::Inspect(format!("client-{first}"))
    );
    assert_eq!(
        admit(
            l,
            &b,
            claim(next, 80, !mixed),
            snapshot(&b, rev, 100),
            Holds::default()
        )
        .await
        .unwrap(),
        Admission::Insufficient
    );
    assert_eq!(
        admit(
            l,
            &b,
            claim(next, 9920, !mixed),
            snapshot(&b, rev, 10000),
            Holds::default()
        )
        .await
        .unwrap(),
        Admission::Send
    );
    let mut other = b.clone();
    other.account = "second".into();
    assert_eq!(epoch(l, &other).await.unwrap(), 0);
    assert!(
        finish(
            l,
            &other,
            first,
            &format!("client-{first}"),
            "order",
            money(32)
        )
        .await
        .is_err()
    );
    drop(db);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn terminal_partial_spend_includes_fee_until_this_balance_books_the_exact_order() {
    let path = file("filled");
    let b = bucket();
    let db = Db::open(&path).unwrap();
    let l = db.ledger();
    assert_eq!(
        admit(
            l,
            &b,
            claim("one", 80, false),
            snapshot(&b, 0, 100),
            Holds::default()
        )
        .await
        .unwrap(),
        Admission::Send
    );
    finish(l, &b, "one", "client-one", "physical-1", money(32))
        .await
        .unwrap();
    finish(l, &b, "one", "client-one", "physical-1", money(32))
        .await
        .unwrap();
    assert!(
        finish(l, &b, "one", "client-one", "physical-1", money(30))
            .await
            .is_err()
    );
    let rev = epoch(l, &b).await.unwrap();
    // Независимый вывод 50 виден, наш IOC списал 30+2. Доступно 18.
    assert_eq!(
        admit(
            l,
            &b,
            claim("blocked", 19, false),
            snapshot(&b, rev, 50),
            Holds::default()
        )
        .await
        .unwrap(),
        Admission::Insufficient
    );
    let mut s = snapshot(&b, rev, 68);
    s.booked.push(Booked {
        bucket: b.clone(),
        order: "unrelated-id".into(),
        debit: money(32),
    });
    assert_eq!(
        admit(l, &b, claim("second", 68, false), s, Holds::default())
            .await
            .unwrap(),
        Admission::Insufficient
    );
    let mut s = snapshot(&b, rev, 68);
    s.booked.push(Booked {
        bucket: b.clone(),
        order: "physical-1".into(),
        debit: money(32),
    });
    assert_eq!(
        admit(l, &b, claim("second", 68, false), s, Holds::default())
            .await
            .unwrap(),
        Admission::Send
    );
    drop(db);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn mixed_open_budget_and_its_cash_order_share_the_original_cap() {
    let b = bucket();
    let c = claim("deal", 80, true);
    let budget = |invested| Holds {
        other: money(20),
        funding: vec![Funding {
            deal: "deal".into(),
            cap: money(80),
            invested: money(invested),
        }],
    };
    let pending = Row {
        claim: c.clone(),
        settled: None,
    };
    assert!(balance::covers(&snapshot(&b, 0, 100), &budget(0), &[pending]).unwrap());
    let settled = [Row {
        claim: c,
        settled: Some(("exchange-1".into(), money(32))),
    }];
    // До и после записи invested в ядре: 20 посторонних + 48 остаток плана + 32 неотражённого списания.
    for invested in [0, 32] {
        assert!(balance::covers(&snapshot(&b, 0, 100), &budget(invested), &settled).unwrap());
        assert!(!balance::covers(&snapshot(&b, 0, 99), &budget(invested), &settled).unwrap());
        let mut s = snapshot(&b, 0, 68);
        s.booked.push(Booked {
            bucket: b.clone(),
            order: "exchange-1".into(),
            debit: money(32),
        });
        assert!(balance::covers(&s, &budget(invested), &settled).unwrap());
    }
}
