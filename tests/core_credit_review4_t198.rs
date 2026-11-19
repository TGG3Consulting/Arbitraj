//! Файл: tests/core_credit_review4_t198.rs
//! Что делает: решение 413 (четвёртое ревью t198) на настоящем ядре с SQLite во временной папке (площадок в реестре
//! нет, моков нет; сделка WKC живого случая 18.11): свежий факт одноразовый и сбрасывается алертом (Б-1); первый вывод
//! ждёт баланс не дольше transfer.max_time_s, Warn только при первом сбое, сделку взял человек — выход (С-4); пауза
//! «баланс не читается» поверх чужой паузы не ставится (С-6); при уходящей со счёта сделке остаток «не присвоено» и
//! добор в тихий момент после её ухода (С-5); «исход неизвестен» — отметка в учёте сделки, счёт «неясно» (С-7); учёт
//! памяти без отметки «не прочитан» (М-5). Остаток после частичной продажи и при закрытии — tests/core_credit_rest_t198.rs.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::gateway; tests/t198_common.

mod t198_common;

use arb::core::credit_alert::{Unread, unread};
use arb::core::deal::Step;
use arb::core::fact_wait::{Waited, first};
use arb::core::types::VenueId;
use arb::settings::value::Scope;
use serde_json::json;
use t198_common::{credited, d, start};

/// Записей журнала вида `kind`.
async fn count(core: &arb::core::Core, kind: &str) -> usize {
    let all = core.inner.deps.journal.list(500, None).await.unwrap();
    all.iter().filter(|e| e.kind == kind).count()
}

/// Настройка биржи mexc.
async fn set(core: &arb::core::Core, name: &str, v: u64) {
    let s = &core.inner.deps.settings;
    s.set(name, Scope::Venue("mexc".into()), Some(json!(v)), "тест")
        .await
        .unwrap();
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

/// С-4: баланс не читается (биржи нет в реестре) — ждём не дольше transfer.max_time_s, повторные чтения без Warn,
/// итог — ошибка «не дождались»; хронология — строка ожидания; сделку взял человек — ожидание кончается.
#[tokio::test]
async fn first_withdraw_waits_with_limit() {
    let core = start("wait").await;
    set(&core, "venue.status_poll_ms", 20).await;
    set(&core, "transfer.max_time_s", 1).await;
    let mut a = credited("W4", "mexc", d("100"));
    let o = arb::core::remain_others::on_account(&core, &a, &a.at);
    let Waited::Read(r, _) = first(&core, &mut a, "нет связи".into(), o).await else {
        panic!("человек не трогал сделку");
    };
    assert!(r.unwrap_err().contains("не дождались"));
    assert!(
        a.timeline
            .iter()
            .any(|(_, t)| t.contains("перед выводом / продажей"))
    );
    assert_eq!(
        count(&core, "core.credit_fact").await,
        0,
        "Warn — только у первого чтения"
    );
    set(&core, "transfer.max_time_s", 30).await;
    let b = credited("W5", "mexc", d("100"));
    let (c, mut x) = (core.clone(), b.clone());
    let h = tokio::spawn(async move {
        let o = arb::core::remain_others::on_account(&c, &x, &x.at);
        matches!(first(&c, &mut x, "нет связи".into(), o).await, Waited::Gone)
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut taken = b.clone();
    taken.step = Step::TakenOver;
    arb::core::ledger::save(&core, &taken).await;
    let gone = tokio::time::timeout(std::time::Duration::from_secs(5), h).await;
    assert!(gone.unwrap().unwrap(), "взял человек — выход");
}

/// С-6: площадка уже на паузе человека — пауза «баланс не читается» её не трогает.
#[tokio::test]
async fn no_fact_over_human_pause_changes_nothing() {
    let core = start("human").await;
    let mexc = VenueId("mexc".into());
    core.inner
        .risk
        .lock()
        .unwrap()
        .venue_paused
        .insert(mexc.clone(), "человек".into());
    arb::core::fact_pause::set(&core, &mexc, "нет связи").await;
    let r = core.inner.risk.lock().unwrap();
    assert_eq!(
        r.venue_paused.get(&mexc).map(String::as_str),
        Some("человек")
    );
    assert!(!r.venue_auto.contains_key(&mexc));
}

/// С-5, пятое ревью: сделка уходит со счёта — доля не выше пула, остаток «не присвоено»; после её ухода (тихий момент
/// счёта) следующее чтение добирает остаток.
#[tokio::test]
async fn rest_is_taken_after_leaving_deal_goes() {
    let core = start("rest").await;
    let mut a = credited("R1", "mexc", d("100"));
    arb::core::ledger::save(&core, &a).await;
    let mut l = credited("L1", "mexc", d("50"));
    (l.credit, l.step, l.transit_to) = (None, Step::Transfer, Some(VenueId("bingx".into())));
    arb::core::ledger::save(&core, &l).await;
    assert!(arb::core::remain_fact::apply_fact(&core, &mut a, d("120")).await);
    assert_eq!(a.qty, d("70"));
    assert_eq!(a.credit.as_deref().unwrap().rest, Some(d("30")));
    arb::core::ledger::save(&core, &a).await;
    l.at = VenueId("bingx".into());
    l.step = Step::Selling;
    arb::core::ledger::save(&core, &l).await;
    // Шестое ревью: решение — под замком счёта (credit_rest::locked; здесь баланс не читается — чистая часть).
    let o = arb::core::remain_others::on_account(&core, &a, &a.at);
    assert!(
        arb::core::credit_rest::busy(&core, &a, &o).is_none(),
        "тихий момент"
    );
    // Восьмое ревью: дележ — credit_rest::share_out (решение под замком при читаемом балансе —
    // tests/core_credit_rest_live_t198.rs).
    let rest = a.credit.as_deref().unwrap().rest.unwrap();
    let q = arb::core::credit_rest::share_out(d("100") - o.qty, &[(a.qty, rest)], 0);
    assert_eq!(q, vec![d("100")], "добрано");
}

/// С-7, пятое ревью (5): продажа с неизвестным исходом — отметка в учёте сделки, счёт «неясно» (замок снят), её
/// монеты — не «подтверждённые» у других. Своя монета теста (замок счёта общий для тестов процесса).
#[tokio::test]
async fn outcome_unknown_marks_account_unsure() {
    use arb::core::op_error::OpError;
    let core = start("unsure").await;
    let mut a = credited("U7", "mexc", d("100"));
    a.direction.asset.ticker = "UNSURE198".into();
    arb::core::credit_fact::confirm(&mut a, d("100"), arb::core::credit_fact::Basis::Balance);
    arb::core::ledger::save(&core, &a).await;
    let e = OpError {
        code: Some(arb::gateway::errors::ErrorCode::OutcomeUnknown),
        message: "исход неизвестен".into(),
        sent: true,
        cancelled: false,
        price_moved: false,
    };
    let r = arb::core::credit_lock::selling(&core, &a, async move { Err(e) }).await;
    assert!(r.is_err());
    let (v, coin) = (a.at.clone(), a.direction.asset.clone());
    assert!(arb::core::credit_lock::unsure(&core, &v, &coin, "U8"));
    assert!(
        arb::core::credit_lock::fact_try(&v, &coin).is_some(),
        "замок снят"
    );
    let mut b = credited("U8", "mexc", d("10"));
    b.direction.asset.ticker = "UNSURE198".into();
    let o = arb::core::remain_others::on_account(&core, &b, &v);
    assert_eq!((o.unclear.clone(), o.qty), (vec!["U7".to_string()], d("0")));
    let x = core.inner.deals.lock().unwrap().get("U7").cloned().unwrap();
    assert!(x.sale_unsure, "в учёте сделки (переживает перезапуск)");
}

/// М-5: в учёт памяти отметка «баланс не прочитан» не попадает — следующий вход получит попытку режима.
#[tokio::test]
async fn memory_copy_has_no_unread_mark() {
    let core = start("m5").await;
    let mut a = credited("M5", "mexc", d("100"));
    a.fact_unread = true;
    arb::core::ledger::save(&core, &a).await;
    assert!(
        !core
            .inner
            .deals
            .lock()
            .unwrap()
            .get("M5")
            .unwrap()
            .fact_unread
    );
}
