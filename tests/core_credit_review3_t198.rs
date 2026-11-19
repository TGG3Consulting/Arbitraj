//! Файл: tests/core_credit_review3_t198.rs
//! Что делает: решение 413 (третье ревью t198) на настоящем ядре с SQLite во временной папке (площадок в реестре нет,
//! моков нет; числа — сделка WKC живого случая 18.11): баланс биржи не прочитан — не продаём вслепую (автоматика:
//! режим застрявшей монеты один раз, потом алерт — Б-3; изнутри режима — сразу алерт, CI 85ebf10c; кнопка — сразу
//! итог кнопки с настоящей причиной — С-Г); пауза новых входов «баланс не читается» и её авто-снятие первым успешным
//! чтением, одна строка Warn на итог чтения (С-Д, С-З); ждущие доли без задачи на круге сверки —
//! доля только запоминается для сверки (С-Б, четвёртое ревью С-2), «больше ожидаемого» — один раз.
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
    // CI 85ebf10c: изнутри режима застрявшей монеты (шаг Stuck) — без повторного входа в режим, сразу человеку.
    let mut c = credited("U3", "mexc", d("100"));
    c.step = Step::Stuck;
    arb::core::ledger::save(&core, &c).await;
    assert!(!unread(&core, &mut c, "лимит запросов", Unread::Stuck).await);
    let rec = core.inner.deals.lock().unwrap().get("U3").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman, "из режима — человеку");
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

/// С-Д, С-З: баланс биржи не читается (площадки нет в реестре) — ошибка, одна строка Warn core.credit_fact, пауза
/// новых входов (NoFact); первое успешное чтение её снимает; чужую паузу (ошибки подряд) не трогает.
#[tokio::test]
async fn unread_pauses_venue_and_read_lifts() {
    let core = start("nofact").await;
    let mexc = VenueId("mexc".into());
    let a = credited("P1", "mexc", d("100"));
    let (r, _) = arb::core::remain_read::read(&core, &a).await;
    assert!(r.is_err());
    // М-8: ветка биржи (Cex) — строка Warn итога чтения и пауза; побочных действий правил шлюза нет (С-З).
    let warn = logged(
        &core,
        "core.credit_fact",
        "баланс не прочитан после 1 попыток",
    )
    .await;
    assert_eq!(warn.len(), 1);
    assert_eq!(logged(&core, "core.credit_fact", "").await.len(), 1);
    assert!(logged(&core, "core.op_error", "").await.is_empty());
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

/// С-Б, четвёртое ревью С-2 и М-4: круг сверки долю НЕ подтверждает — только запоминает для сверки (ждущая
/// человека считается долей по балансу, не нижней оценкой — расхождения и паузы нет); «больше ожидаемого» — Warn один
/// раз на (сделка, биржа, запись, баланс); сделка перестала ждать доли — память чистится.
#[tokio::test]
async fn reconcile_round_only_observes() {
    let core = start("halted").await;
    let mexc = VenueId("mexc".into());
    let mut a = credited("H1", "mexc", d("100"));
    a.step = Step::WaitingHuman;
    arb::core::ledger::save(&core, &a).await;
    let key = arb::core::reconcile_key::key(&core, &a.direction.asset, &mexc);
    for _ in 0..2 {
        arb::core::credit_halted::observe(&core, &mexc, &key, d("90")).await;
    }
    let rec = core.inner.deals.lock().unwrap().get("H1").cloned().unwrap();
    assert!(arb::core::credit_fact::pending(&rec), "не подтверждено");
    let warn = logged(&core, "core.credit_fact", "долю не подтверждает").await;
    assert_eq!(warn.len(), 1);
    assert!(warn[0].repeats.is_none(), "один раз, не каждый круг");
    assert_eq!(arb::core::credit_halted::observed(&rec), Some(d("90")));
    let m = arb::core::reconcile_confirm::expected(&core, &mexc, None);
    let need: Decimal = m.values().map(|e| e.0).sum();
    assert_eq!(need, d("90"), "доля по балансу — расхождения нет");
    // Полное зачисление у остановленной — тоже только память, учёт не тронут.
    let mut b = credited("W1", "bingx", d("100"));
    b.step = Step::Halted;
    arb::core::ledger::save(&core, &b).await;
    let bingx = VenueId("bingx".into());
    let key = arb::core::reconcile_key::key(&core, &b.direction.asset, &bingx);
    arb::core::credit_halted::observe(&core, &bingx, &key, d("100")).await;
    let rec = core.inner.deals.lock().unwrap().get("W1").cloned().unwrap();
    assert!(
        arb::core::credit_fact::pending(&rec),
        "подтверждает только факт под замком"
    );
    assert_eq!(arb::core::credit_halted::observed(&rec), Some(d("100")));
    // H1 подтверждена (перед продажей) — память о ней чистится на следующем круге.
    let mut h = core.inner.deals.lock().unwrap().get("H1").cloned().unwrap();
    arb::core::credit_fact::confirm(&mut h, d("90"), arb::core::credit_fact::Basis::Balance);
    arb::core::ledger::save(&core, &h).await;
    arb::core::credit_halted::observe(&core, &bingx, &key, d("100")).await;
    h.credit.as_deref_mut().unwrap().share = None;
    assert_eq!(
        arb::core::credit_halted::observed(&h),
        None,
        "память забыта"
    );
}
