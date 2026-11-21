//! Файл: tests/core_credit_rest_live2_t198.rs
//! Что делает: решение 413 (восьмое ревью t198) на стенде с читаемым балансом (tests/t198_live: сервер по формату
//! официальной документации XT, справочник — живая запись, баланс задаёт тест): своя сделка с неизвестным исходом
//! продажи в совместном решении — база 0, подтверждённое соучастника не урезано (С-1); круг сверки при первой сделке
//! монеты «занята» пробует следующую (С-2); ветка «уже решён другой сделкой» переносит количество и хронологию (С-4);
//! соучастник в ожидании после продажи (количество 0) решается совместно; строка «решён…» остаётся в хронологии учёта.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core; tests/t198_common, tests/t198_live.

mod t198_common;
mod t198_live;

use arb::core::Core;
use arb::core::credit_fact::{Basis, confirm, rest, set_rest};
use arb::core::credit_rest::{Res, locked};
use arb::core::deal::{Deal, Step};
use t198_common::{credited, d};
use t198_live::{big, start};

/// Сделка BTC на xt: запись 1, подтверждено `qty`, остаток `r`, шаг `step`.
fn deal(id: &str, qty: &str, r: &str, step: Step) -> Deal {
    let mut a = credited(id, "xt", d("1"));
    a.direction.asset.ticker = "BTC".into();
    confirm(&mut a, d(qty), Basis::Balance);
    set_rest(&mut a, d(r));
    a.step = step;
    a
}

/// Сделка из учёта памяти.
fn mem(core: &Core, id: &str) -> Deal {
    core.inner.deals.lock().unwrap().get(id).cloned().unwrap()
}

/// С-1: своя D2 с неизвестным исходом продажи (0.3 + 0.1) и соучастник D1 (0.4 + 0.1), на счёте 0.5: D1 получает своё
/// 0.4 и долю добавки — не урезана; D2 — только из оставшегося.
#[test]
fn unsure_self_never_cuts_peer() {
    big(async {
        let (core, _) = start(&t198_common::dir("live2-unsure"), "0.50").await;
        let d1 = deal("D1", "0.40", "0.10", Step::Halted);
        arb::core::ledger::save(&core, &d1).await;
        let mut d2 = deal("D2", "0.30", "0.10", Step::Selling);
        d2.sale_unsure = true;
        arb::core::ledger::save(&core, &d2).await;
        assert!(matches!(locked(&core, &mut d2, false).await.res, Res::Done));
        assert_eq!(d2.qty, d("0.08"));
        let x = mem(&core, "D1");
        assert_eq!(
            (x.qty, rest(&x)),
            (d("0.42"), None),
            "подтверждённое D1 не урезано"
        );
    });
}

/// С-2: у первой сделки монеты счёт «занят» (у другой — неизвестный исход), у второй тихо: круг сверки пробует вторую
/// и решает обе совместно — при любом порядке. Строка «решён…» — в хронологии учёта.
#[test]
fn round_tries_next_deal_of_coin() {
    big(async {
        let (core, _) = start(&t198_common::dir("live2-order"), "1").await;
        let a = deal("O1", "0.2", "0.3", Step::Halted);
        let mut b = deal("O2", "0.1", "0.2", Step::Halted);
        b.sale_unsure = true;
        arb::core::ledger::save(&core, &a).await;
        arb::core::ledger::save(&core, &b).await;
        arb::core::credit_rest_park::round(&core, &a.at).await;
        let (x, y) = (mem(&core, "O1"), mem(&core, "O2"));
        assert_eq!((rest(&x), rest(&y)), (None, None), "обе решены");
        assert!(
            x.timeline
                .iter()
                .any(|(_, t)| t.contains("Остаток зачисления решён"))
        );
        assert!(
            y.timeline
                .iter()
                .any(|(_, t)| t.contains("Остаток зачисления решён"))
        );
    });
}

/// С-4 и соучастник в ожидании после продажи: A (количество 0, остаток 0.3 — ждёт тихий момент) решается совместно
/// кругом сверки по B; затем ожидание A берёт решение из памяти (количество и хронология), без второго решения.
#[test]
fn waiting_peer_and_resolved_by_other() {
    big(async {
        let (core, _) = start(&t198_common::dir("live2-peer"), "1").await;
        let mut a = deal("A1", "0", "0.3", Step::Selling);
        arb::core::ledger::save(&core, &a).await;
        let b = deal("B1", "0.2", "0.1", Step::Halted);
        arb::core::ledger::save(&core, &b).await;
        arb::core::credit_rest_park::round(&core, &b.at).await;
        let x = mem(&core, "A1");
        assert_eq!((x.qty, rest(&x)), (d("0.3"), None), "решена совместно");
        let l = locked(&core, &mut a, false).await;
        assert!(matches!(l.res, Res::Done));
        assert_eq!((a.qty, rest(&a)), (d("0.3"), None));
        assert!(a.timeline.iter().any(|(_, t)| t.contains("совместно с")));
    });
}
