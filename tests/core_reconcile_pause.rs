//! Файл: tests/core_reconcile_pause.rs
//! Что проверяет: решение 221 — чистая логика автоснятия «Паузы» от сверки балансов (core/reconcile_pause.rs):
//! 1) пауза снимается только после ledger.reconcile_confirm_rounds чистых кругов ПОДРЯД (круг с расхождением
//!    обнуляет счёт), не задано / 0 / 1 — с первого чистого круга;
//! 2) пауза не сверки (человек, другой модуль, режим меняли — отметки нет) не снимается и счёт не копит;
//! 3) «чистый круг» — нет ни одной проблемы risk.reconcile_mismatch: ни подтверждённой, ни ждущей кругов, ни «балансы
//!    не получены»; прочие проблемы (мало BNB, мало USDT) не мешают;
//! 4) текст для журнала о паузе — расхождения монет (без «балансы не получены» — это пауза площадки).
//!    Пример — случай 08.11: «PORTO по учёту 119.25, на площадке 0».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::reconcile_pause, arb::core::reconcile (Found), reconcile_confirm (UNAVAILABLE).

use arb::core::reconcile::Found;
use arb::core::reconcile_confirm::UNAVAILABLE;
use arb::core::reconcile_pause::{Lift, is_clean, mismatch_text};

/// Расхождение монеты (ключ «площадка/монета»).
fn coin(text: &str) -> Found {
    (
        "risk.reconcile_mismatch",
        Some("mexc".into()),
        text.into(),
        Some("mexc/PORTO".into()),
    )
}

/// «Балансы площадки не получены».
fn unavailable() -> Found {
    (
        "risk.reconcile_mismatch",
        Some("bingx".into()),
        "bingx: балансы не получены — таймаут".into(),
        Some(format!("bingx{UNAVAILABLE}")),
    )
}

#[test]
fn lifts_after_need_clean_rounds_in_a_row() {
    let mut l = Lift::default();
    assert!(!l.round(true, true, 3));
    assert!(!l.round(true, true, 3));
    assert_eq!(l.clean(), 2);
    // Круг с расхождением — счёт сначала.
    assert!(!l.round(true, false, 3));
    assert_eq!(l.clean(), 0);
    assert!(!l.round(true, true, 3));
    assert!(!l.round(true, true, 3));
    assert!(l.round(true, true, 3), "третий чистый подряд — снять");
    assert_eq!(l.clean(), 0, "после снятия счёт сначала");
}

#[test]
fn need_zero_or_one_lifts_on_first_clean_round() {
    for need in [0, 1] {
        let mut l = Lift::default();
        assert!(l.round(true, true, need), "need = {need}");
    }
}

#[test]
fn not_reconcile_pause_never_lifts() {
    let mut l = Lift::default();
    for _ in 0..5 {
        assert!(!l.round(false, true, 1));
    }
    assert_eq!(l.clean(), 0);
    // Отметку сняли сменой режима посреди счёта — счёт обнулён, снятия нет.
    assert!(!l.round(true, true, 2));
    assert!(!l.round(false, true, 2));
    assert!(!l.round(true, true, 2), "счёт начат заново");
    assert!(l.round(true, true, 2));
}

#[test]
fn clean_round_rules() {
    assert!(is_clean(&[]));
    let other: Found = ("risk.low_bnb", Some("bsc".into()), "мало BNB".into(), None);
    assert!(is_clean(&[other]), "мало BNB — не расхождение");
    assert!(!is_clean(&[coin("x")]));
    assert!(!is_clean(&[unavailable()]), "не сверили — круг не чистый");
}

#[test]
fn mismatch_text_for_journal() {
    let porto = "mexc: PORTO по учёту 119.25, на площадке 0";
    let t = mismatch_text(&[unavailable(), coin(porto)]);
    assert_eq!(
        t, porto,
        "«балансы не получены» — пауза площадки, не в тексте"
    );
    assert!(mismatch_text(&[unavailable()]).is_empty());
    let two = mismatch_text(&[coin("a"), coin("b")]);
    assert_eq!(two, "a; b");
}
