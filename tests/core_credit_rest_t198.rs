//! Файл: tests/core_credit_rest_t198.rs
//! Что делает: решение 413 (пятое ревью t198) на настоящем ядре с SQLite во временной папке (площадок в реестре нет,
//! моков нет): остаток «не присвоено» (CreditFact.rest) не теряется. Б-1: после частичной продажи и с посторонними на
//! счёте добор в тихий момент не выше «количество + остаток» и не трогает подтверждённые монеты других сделок. Б-2:
//! закрытие с остатком — сделка остаётся отдельной позицией (остановлена, количество = остаток, журнал Warn); продажа
//! кончилась, а тихий момент не наступил к пределу transfer.max_time_s — то же.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage; tests/t198_common.

mod t198_common;

use arb::core::credit_fact::{Basis, confirm, set_rest};
use arb::core::deal::Step;
use arb::settings::value::Scope;
use serde_json::json;
use t198_common::{credited, d, start};

/// Б-1: доля 70, остаток 30, продано 50 (осталось 20); на счёте 20 + 30 своих + 40 другой сделки + 25 посторонних = 115.
/// Тихий момент: доля = min(20 + 30, 115 − 40) = 50 — не выше «количество + остаток», монеты другой сделки не тронуты.
#[tokio::test]
async fn rest_after_partial_sale_and_outsiders() {
    let core = start("rest-b1").await;
    let mut a = credited("B1", "mexc", d("100"));
    confirm(&mut a, d("70"), Basis::Balance);
    set_rest(&mut a, d("30"));
    a.qty = d("20");
    arb::core::ledger::save(&core, &a).await;
    let mut c = credited("C1", "mexc", d("40"));
    confirm(&mut c, d("40"), Basis::Balance);
    arb::core::ledger::save(&core, &c).await;
    assert!(arb::core::remain_fact::apply_fact(&core, &mut a, d("115")).await);
    assert_eq!(a.qty, d("50"));
    assert_eq!(arb::core::credit_fact::rest(&a), None);
    let rec = core.inner.deals.lock().unwrap().get("C1").cloned().unwrap();
    assert_eq!(rec.qty, d("40"), "чужие подтверждённые не тронуты");
}

/// Б-1: не тихий момент (через счёт идёт другая ждущая доли) — остаток не решается, сделка ведётся долей.
#[tokio::test]
async fn rest_waits_while_account_is_busy() {
    let core = start("rest-busy").await;
    let mut a = credited("B2", "mexc", d("100"));
    confirm(&mut a, d("70"), Basis::Balance);
    set_rest(&mut a, d("30"));
    arb::core::ledger::save(&core, &a).await;
    let p = credited("P2", "mexc", d("10"));
    arb::core::ledger::save(&core, &p).await;
    arb::core::remain_fact::apply_fact(&core, &mut a, d("200")).await;
    assert_eq!(a.qty, d("70"));
    assert_eq!(arb::core::credit_fact::rest(&a), Some(d("30")));
}

/// Б-2: закрытие с остатком — остаётся отдельной позицией (остановлена, количество = остаток), Warn с цифрами.
#[tokio::test]
async fn close_with_rest_keeps_position() {
    let core = start("rest-close").await;
    let mut a = credited("B3", "mexc", d("100"));
    confirm(&mut a, d("98"), Basis::Balance);
    set_rest(&mut a, d("2"));
    a.qty = d("0");
    arb::core::ledger::finish(&core, &mut a, Step::Closed, "продано").await;
    let rec = core.inner.deals.lock().unwrap().get("B3").cloned().unwrap();
    assert_eq!((rec.step, rec.qty), (Step::Halted, d("2")));
    let all = core.inner.deps.journal.list(500, None).await.unwrap();
    assert!(
        all.iter()
            .any(|e| e.kind == "core.credit_fact" && e.message.contains("не присвоено"))
    );
}

/// Б-2: продажа кончилась, баланс не читается (биржи нет в реестре) — тихий момент не наступил к пределу
/// transfer.max_time_s: остаток — отдельная позиция.
#[tokio::test]
async fn at_close_limit_parks_rest() {
    let core = start("rest-limit").await;
    let s = &core.inner.deps.settings;
    for (n, v) in [("venue.status_poll_ms", 20), ("transfer.max_time_s", 1)] {
        s.set(n, Scope::Venue("mexc".into()), Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    let mut a = credited("B4", "mexc", d("100"));
    confirm(&mut a, d("98"), Basis::Balance);
    set_rest(&mut a, d("2"));
    a.qty = d("0");
    arb::core::ledger::save(&core, &a).await;
    arb::core::credit_rest::at_close(&core, a, "выгодно".into()).await;
    let rec = core.inner.deals.lock().unwrap().get("B4").cloned().unwrap();
    assert_eq!((rec.step, rec.qty), (Step::Halted, d("2")));
}
