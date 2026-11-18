//! Файл: tests/core_credit_review3_t198.rs
//! Что делает: решение 413 (третье ревью t198) на настоящем ядре с SQLite во временной папке (площадок в реестре нет,
//! моков нет; числа — сделка WKC живого случая 18.11): баланс биржи не прочитан — не продаём вслепую (автоматика:
//! режим застрявшей монеты один раз, потом алерт — Б-3; кнопка — сразу итог кнопки с настоящей причиной — С-Г; после
//! алерта свежего факта нет — кнопка читает факт — Б-1); пауза новых входов «баланс не читается» и её авто-снятие
//! первым успешным чтением, одна строка Warn на итог чтения (С-Д, С-З); ждущие доли без задачи на круге сверки —
//! «больше ожидаемого» не подтверждается, сообщается один раз на (сделка, баланс), сверка считает её долей по факту;
//! ждущая человека подтверждается (С-Б).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::storage; tests/t198_common.

mod t198_common;

use arb::core::auto_pause::Kind;
use arb::core::credit_alert::{Unread, unread};
use arb::core::deal::Step;
use arb::core::types::VenueId;
use rust_decimal::Decimal;
use t198_common::{credited, d, start};

/// Записи журнала вида `kind`, текст которых содержит `has`.
async fn logged(
    core: &arb::core::Core,
    kind: &str,
    has: &str,
) -> Vec<arb::storage::journal::Entry> {
    let all = core.inner.deps.journal.list(500, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == kind && e.message.contains(has))
        .collect()
}

/// Б-3: автоматика — первый раз режим застрявшей монеты (stuck.mode не задан — остановка с причиной), повторно
/// подряд — алерт человеку; монеты не проданы.
#[tokio::test]
async fn unread_balance_never_sells_blind() {
    let core = start("unread").await;
    let mut a = credited("U1", "mexc", d("100"));
    arb::core::ledger::save(&core, &a).await;
    assert!(!unread(&core, &mut a, "лимит запросов", Unread::Stuck).await);
    assert!(a.fact_unread);
    let rec = core.inner.deals.lock().unwrap().get("U1").cloned().unwrap();
    assert_eq!(
        rec.step,
        Step::Halted,
        "без stuck.mode — остановка с причиной"
    );
    assert_eq!(rec.qty, d("100"), "не продано");
    let mut b = credited("U2", "mexc", d("100"));
    b.fact_unread = true;
    arb::core::ledger::save(&core, &b).await;
    assert!(!unread(&core, &mut b, "лимит запросов", Unread::Stuck).await);
    let rec = core.inner.deals.lock().unwrap().get("U2").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman, "повторно — человеку");
}

/// С-Г: кнопка человека — сразу итог кнопки «Не продано: баланс биржи не прочитан …» (журнал core.manual_sale) и
/// алерт с той же причиной; без режима застрявшей монеты.
#[tokio::test]
async fn button_unread_gives_real_reason_at_once() {
    let core = start("button").await;
    let mut a = credited("G1", "mexc", d("100"));
    arb::core::ledger::save(&core, &a).await;
    assert!(!unread(&core, &mut a, "лимит запросов", Unread::Button).await);
    let rec = core.inner.deals.lock().unwrap().get("G1").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman);
    assert_eq!(rec.qty, d("100"));
    let out = logged(&core, "core.manual_sale", "баланс биржи не прочитан").await;
    assert_eq!(out.len(), 1, "итог кнопки с настоящей причиной");
}

/// Б-1: свежий факт одноразовый — второй вход подряд его забирает, третий читает снова; другое количество — не свежий;
/// после алерта (повтор «не прочитан») в памяти свежего факта нет — кнопка прочитает факт.
#[tokio::test]
async fn fresh_fact_is_one_shot_and_cleared_by_alert() {
    use arb::core::sell_fact::fresh_take;
    let core = start("fresh").await;
    let mut a = credited("F1", "mexc", d("100"));
    a.fact_fresh = Some(Box::new((a.step, a.qty, a.at.clone())));
    assert!(fresh_take(&mut a));
    assert!(!fresh_take(&mut a), "второй раз — читать снова");
    a.fact_fresh = Some(Box::new((a.step, d("99"), a.at.clone())));
    assert!(!fresh_take(&mut a), "количество другое — не свежий");
    a.fact_fresh = Some(Box::new((a.step, a.qty, a.at.clone())));
    a.fact_unread = true;
    arb::core::ledger::save(&core, &a).await;
    assert!(!unread(&core, &mut a, "лимит запросов", Unread::Stuck).await);
    let mut rec = core.inner.deals.lock().unwrap().get("F1").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman);
    assert!(rec.fact_fresh.is_none());
    assert!(!fresh_take(&mut rec), "кнопка после алерта читает факт");
}

/// С-Д, С-З: баланс биржи не читается (площадки нет в реестре) — ошибка, одна строка Warn core.credit_fact, пауза
/// новых входов (NoFact); первое успешное чтение её снимает; чужую паузу (ошибки подряд) не трогает.
#[tokio::test]
async fn unread_pauses_venue_and_read_lifts() {
    let core = start("nofact").await;
    let mexc = VenueId("mexc".into());
    let a = credited("P1", "mexc", d("100"));
    let (r, _) = arb::core::remain_read::read(&core, &a).await;
    assert!(r.is_err());
    assert_eq!(logged(&core, "core.credit_fact", "").await.len(), 1);
    let kind = core
        .inner
        .risk
        .lock()
        .unwrap()
        .venue_auto
        .get(&mexc)
        .map(|m| m.kind);
    assert_eq!(kind, Some(Kind::NoFact));
    assert!(
        core.inner
            .risk
            .lock()
            .unwrap()
            .venue_paused
            .contains_key(&mexc)
    );
    arb::core::fact_pause::lift(&core, &mexc).await;
    assert!(
        !core
            .inner
            .risk
            .lock()
            .unwrap()
            .venue_paused
            .contains_key(&mexc)
    );
    {
        let mut r = core.inner.risk.lock().unwrap();
        arb::core::auto_pause::pause_venue(
            &mut r,
            &mexc,
            "ошибки".into(),
            Kind::Errors,
            chrono::Utc::now(),
        );
    }
    arb::core::fact_pause::lift(&core, &mexc).await;
    assert!(
        core.inner
            .risk
            .lock()
            .unwrap()
            .venue_paused
            .contains_key(&mexc),
        "чужая пауза"
    );
}

/// С-Б: ждущая человека с «больше ожидаемого» не подтверждается; Warn — один раз на (сделка, баланс); сверка считает
/// её долей по факту (не нижней оценкой — расхождения и паузы нет). Остановленная при полном зачислении подтверждается.
#[tokio::test]
async fn halted_and_waiting_on_reconcile_round() {
    let core = start("halted").await;
    let mexc = VenueId("mexc".into());
    let mut a = credited("H1", "mexc", d("100"));
    a.step = Step::WaitingHuman;
    arb::core::ledger::save(&core, &a).await;
    let key = arb::core::reconcile_key::key(&core, &a.direction.asset, &mexc);
    for _ in 0..2 {
        arb::core::credit_halted::confirm(&core, &mexc, &key, d("90")).await;
    }
    let rec = core.inner.deals.lock().unwrap().get("H1").cloned().unwrap();
    assert!(arb::core::credit_fact::pending(&rec), "не подтверждено");
    let warn = logged(&core, "core.credit_fact", "на круге сверки не подтверждено").await;
    assert_eq!(warn.len(), 1);
    assert!(warn[0].repeats.is_none(), "один раз, не каждый круг");
    assert_eq!(arb::core::credit_halted::observed("H1"), Some(d("90")));
    let m = arb::core::reconcile_confirm::expected(&core, &mexc, None);
    let need: Decimal = m.values().map(|e| e.0).sum();
    assert_eq!(need, d("90"), "доля по факту — расхождения нет");
    let mut b = credited("W1", "bingx", d("100"));
    b.step = Step::Halted;
    arb::core::ledger::save(&core, &b).await;
    let bingx = VenueId("bingx".into());
    let key = arb::core::reconcile_key::key(&core, &b.direction.asset, &bingx);
    arb::core::credit_halted::confirm(&core, &bingx, &key, d("100")).await;
    let rec = core.inner.deals.lock().unwrap().get("W1").cloned().unwrap();
    assert!(!arb::core::credit_fact::pending(&rec));
    assert_eq!(rec.step, Step::Halted, "шаг не тронут");
    assert_eq!(rec.qty, d("100"));
}
