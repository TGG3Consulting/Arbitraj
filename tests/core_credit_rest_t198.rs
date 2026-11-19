//! Файл: tests/core_credit_rest_t198.rs
//! Что делает: решение 413 (пятое и шестое ревью t198) на настоящем ядре с SQLite во временной папке (площадок в
//! реестре нет — баланс не читается; моков нет): остаток «не присвоено» (CreditFact.rest) не теряется и не становится
//! фантомом. Б-1: остаток без удержанного биржей; закрытие с остатком — сделка остановлена с подтверждённым количеством,
//! остаток отдельно (не подтверждён), сверка паузы не даёт; «монет нет» — фантом закрывается. Добор в тихий момент
//! после частичной продажи и с посторонними — не выше «количество + остаток», чужие не тронуты. Б-2: сделка с
//! остатком и количеством 0 видна другим как «неясно»; «Беру на себя» на счёте — вычитается. С-1: перезапуск во время
//! ожидания — продажа и остаток в учёте. С-2: «Продано вручную» — остаток к человеку. С-3: «Продать сейчас» при
//! занятом счёте — «счёт занят». Предел ожидания — отдельная позиция.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage; tests/t198_common.

mod t198_common;

use arb::core::credit_fact::{Basis, confirm, rest, set_rest};
use arb::core::deal::{Deal, Step};
use arb::core::types::VenueId;
use arb::settings::value::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use t198_common::{credited, d, start};

/// Сделка с подтверждённым количеством `qty` и остатком `r` на mexc.
fn with_rest(id: &str, qty: &str, r: &str) -> Deal {
    let mut a = credited(id, "mexc", d("100"));
    confirm(&mut a, d(qty), Basis::Balance);
    set_rest(&mut a, d(r));
    a
}

/// Б-1: после частичной продажи (осталось 20 из 70, остаток 30) и с посторонними (на счёте 115, у другой сделки 40)
/// тихий момент даёт min(20 + 30, 115 − 40) = 50; чужие подтверждённые не тронуты.
#[tokio::test]
async fn rest_after_partial_sale_and_outsiders() {
    let core = start("rest-b1").await;
    let mut a = with_rest("B1", "70", "30");
    a.qty = d("20");
    arb::core::ledger::save(&core, &a).await;
    let mut c = credited("C1", "mexc", d("40"));
    confirm(&mut c, d("40"), Basis::Balance);
    arb::core::ledger::save(&core, &c).await;
    let o = arb::core::remain_others::on_account(&core, &a, &a.at);
    assert!(arb::core::credit_rest::busy(&core, &a, &o).is_none());
    arb::core::credit_rest::resolve(&mut a, d("115"), &o, d("0"));
    assert_eq!((a.qty, rest(&a)), (d("50"), None));
    assert_eq!(o.qty, d("40"), "чужие подтверждённые вычтены, не выданы");
}

/// Б-1: остаток при сделке, уходящей со счёта, — до записи × (1 − ожидаемое удержание), удержанное биржей не входит.
#[tokio::test]
async fn rest_excludes_expected_hold() {
    let core = start("rest-hold").await;
    let mut a = credited("H2", "mexc", d("100"));
    let s = arb::core::deposit_hold::Sample {
        records: d("100"),
        credited: d("98"),
        pct: d("2"),
        at: chrono::Utc::now(),
        deals: vec!["H0".into()],
    };
    arb::core::deposit_hold::put(&core, &a.at, &a.direction.asset, s);
    arb::core::ledger::save(&core, &a).await;
    let mut l = credited("L2", "mexc", d("50"));
    (l.credit, l.step, l.transit_to) = (None, Step::Transfer, Some(VenueId("bingx".into())));
    arb::core::ledger::save(&core, &l).await;
    assert!(arb::core::remain_fact::apply_fact(&core, &mut a, d("120")).await);
    assert_eq!((a.qty, rest(&a)), (d("70"), Some(d("28"))));
}

/// Б-1: закрытие с остатком — остановлена с подтверждённым количеством (0), остаток отдельно и не подтверждён; в
/// сверке такой остаток не числится (паузы нет); «монет нет» у сделки с остатком — фантом закрывается.
#[tokio::test]
async fn close_with_rest_keeps_unconfirmed_rest() {
    let core = start("rest-close").await;
    let mut a = with_rest("B3", "98", "2");
    a.qty = d("0");
    arb::core::ledger::finish(&core, &mut a, Step::Closed, "продано").await;
    let rec = core.inner.deals.lock().unwrap().get("B3").cloned().unwrap();
    assert_eq!(
        (rec.step, rec.qty, rest(&rec)),
        (Step::Halted, d("0"), Some(d("2")))
    );
    let m = arb::core::reconcile_confirm::expected(&core, &rec.at, None);
    assert_eq!(
        m.values().map(|e| e.0).sum::<Decimal>(),
        d("0"),
        "сверка не ждёт остаток"
    );
    // «Монет нет»: счёт не тихий (другая ждёт доли) и пуст — фантом закрывается.
    let p = credited("P3", "mexc", d("10"));
    arb::core::ledger::save(&core, &p).await;
    let mut b = with_rest("B4", "5", "3");
    arb::core::ledger::save(&core, &b).await;
    assert!(!arb::core::remain_fact::apply_fact(&core, &mut b, d("0")).await);
    assert_eq!((b.step, rest(&b)), (Step::Closed, None));
}

/// Б-2: сделка с остатком и количеством 0 (ожидание после продажи) — другим «неясно»; «Беру на себя» на счёте —
/// вычитается.
#[tokio::test]
async fn others_see_rest_and_taken_over() {
    let core = start("rest-b2").await;
    let mut a = with_rest("A5", "98", "2");
    a.qty = d("0");
    a.step = Step::Selling;
    arb::core::ledger::save(&core, &a).await;
    let b = with_rest("B5", "10", "3");
    arb::core::ledger::save(&core, &b).await;
    let o = arb::core::remain_others::on_account(&core, &b, &b.at);
    assert_eq!(o.unclear, vec!["A5".to_string()]);
    assert!(
        arb::core::credit_rest::busy(&core, &b, &o).is_some(),
        "у другой остаток — не тихо"
    );
    let h = arb::core::types::ManualHolding {
        id: "h5".into(),
        deal_id: "T5".into(),
        venue: b.at.clone(),
        asset: b.direction.asset.clone(),
        qty: d("7"),
        reason: "тест".into(),
        taken_at: chrono::Utc::now(),
    };
    let t = credited("T5", "mexc", d("7"));
    core.inner
        .holdings
        .lock()
        .unwrap()
        .insert("h5".into(), (h, t));
    let o = arb::core::remain_others::on_account(&core, &b, &b.at);
    assert_eq!(o.qty, d("7"), "монеты человека вычитаются");
}

/// С-1: перезапуск во время ожидания тихого момента — продажа и остаток в учёте: сделка остановлена с остатком.
#[tokio::test]
async fn restart_during_wait_keeps_rest() {
    let core = start("rest-c1").await;
    let mut a = with_rest("R6", "98", "2");
    a.qty = d("0");
    a.step = Step::Selling;
    arb::core::ledger::save(&core, &a).await;
    arb::core::ledger_load::load(&core).await;
    let rec = core.inner.deals.lock().unwrap().get("R6").cloned().unwrap();
    assert_eq!((rec.step, rest(&rec)), (Step::Halted, Some(d("2"))));
    assert!(
        core.halted().iter().any(|x| x.id == "R6"),
        "в «Ручном управлении»"
    );
}

/// С-2: «Продано вручную» у сделки с остатком — остаток к человеку, сделка закрыта.
#[tokio::test]
async fn sold_manually_takes_rest() {
    let core = start("rest-c2").await;
    let mut a = with_rest("S7", "1", "2");
    a.step = Step::Halted;
    arb::core::ledger::save(&core, &a).await;
    let r = arb::core::manual_halted::sold_manually(&core, "S7", "5", "тест").await;
    assert!(r.unwrap().contains("закрыта"));
    let rec = core.inner.deals.lock().unwrap().get("S7").cloned().unwrap();
    assert_eq!((rec.step, rest(&rec)), (Step::Closed, None));
    assert!(
        rec.timeline.iter().any(|(_, t)| t.contains("3 WKC")),
        "количество с остатком"
    );
}

/// С-3: «Продать сейчас» у остановленной с остатком при занятом счёте (другая ждёт доли) — «счёт занят», сделка
/// остаётся остановленной. Предел ожидания после продажи — отдельная позиция.
#[tokio::test]
async fn sell_now_busy_and_wait_limit() {
    let core = start("rest-c3").await;
    let mut a = with_rest("N8", "5", "2");
    a.step = Step::Halted;
    arb::core::ledger::save(&core, &a).await;
    let p = credited("P8", "mexc", d("10"));
    arb::core::ledger::save(&core, &p).await;
    let e = arb::core::manual_halted::sell_halted(&core, "N8", false)
        .await
        .unwrap_err();
    assert!(e.contains("счёт занят"), "{e}");
    let rec = core.inner.deals.lock().unwrap().get("N8").cloned().unwrap();
    assert_eq!(rec.step, Step::Halted);
    let s = &core.inner.deps.settings;
    for (n, v) in [("venue.status_poll_ms", 20), ("transfer.max_time_s", 1)] {
        s.set(n, Scope::Venue("mexc".into()), Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    let mut b = with_rest("W8", "98", "2");
    b.qty = d("0");
    arb::core::ledger::save(&core, &b).await;
    arb::core::credit_rest::at_close(&core, b, "выгодно".into()).await;
    let rec = core.inner.deals.lock().unwrap().get("W8").cloned().unwrap();
    assert_eq!(
        (rec.step, rec.qty, rest(&rec)),
        (Step::Halted, d("0"), Some(d("2")))
    );
}
