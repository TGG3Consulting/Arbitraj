//! Файл: tests/core_credit_rest2_t198.rs
//! Что делает: решение 413 (восьмое ревью t198) — дележ пула между сделками с остатком (credit_rest::share_out:
//! подтверждённые соучастников не урезаются, своя с неизвестным исходом — база 0, ограничение «количество + остаток»)
//! и «Беру на себя» при занятом счёте — отказ с причиной, алерт и остаток остаются (настоящее ядро на SQLite во
//! временной папке, площадок в реестре нет).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core; tests/t198_common.

mod t198_common;

use arb::core::credit_fact::{Basis, confirm, rest, set_rest};
use t198_common::{credited, d, start};

/// Сделка с подтверждённым количеством `qty` и остатком `r` на mexc.
fn with_rest(id: &str, qty: &str, r: &str) -> arb::core::deal::Deal {
    let mut a = credited(id, "mexc", d("100"));
    confirm(&mut a, d(qty), Basis::Balance);
    set_rest(&mut a, d(r));
    a
}

/// Восьмое ревью: дележ — подтверждённые соучастников не урезаются никогда (пул меньше их баз — своей 0, им — их);
/// своя с неизвестным исходом — база 0; добавка по остаткам, каждой не больше «количество + остаток».
#[test]
fn share_out_rules() {
    use arb::core::credit_rest::share_out;
    let parts = [(d("0.5"), d("0.3")), (d("0.4"), d("0.1"))];
    assert_eq!(
        share_out(d("0.6"), &parts, 2),
        vec![d("0.2"), d("0.4")],
        "пул < баз — свою"
    );
    assert_eq!(
        share_out(d("0.3"), &parts, 2),
        vec![d("0"), d("0.4")],
        "соучастника не урезаем"
    );
    assert_eq!(
        share_out(d("10"), &parts, 2),
        vec![d("0.8"), d("0.5")],
        "не больше количества + остатка"
    );
    let unsure = [(d("0"), d("0.4")), (d("0.4"), d("0.1"))];
    assert_eq!(share_out(d("0.5"), &unsure, 2), vec![d("0.08"), d("0.42")]);
}

/// Восьмое ревью (С-3): «Беру на себя» при занятом счёте — отказ с причиной, алерт остаётся, остаток не обнулён.
#[tokio::test]
async fn take_over_busy_keeps_rest() {
    let core = start("rest-take").await;
    let a = with_rest("T9", "1", "2");
    arb::core::ledger::save(&core, &a).await;
    arb::core::manual::raise_alert(&core, a, None, "тест".into(), None).await;
    let id = {
        let m = core.inner.alerts.lock().unwrap();
        m.values()
            .find(|(_, d)| d.id == "T9")
            .map(|(x, _)| x.id.clone())
            .unwrap()
    };
    let p = credited("P9", "mexc", d("10"));
    arb::core::ledger::save(&core, &p).await;
    let e = arb::core::manual::take_over(&core, &id, "тест")
        .await
        .unwrap_err();
    assert!(e.contains("Беру на себя") && e.contains("2 WKC"), "{e}");
    let m = core.inner.alerts.lock().unwrap();
    let (_, x) = m.get(&id).expect("алерт остаётся");
    assert_eq!(rest(x), Some(d("2")));
}
