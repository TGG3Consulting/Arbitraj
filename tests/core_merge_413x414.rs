//! Файл: tests/core_merge_413x414.rs
//! Что проверяет: ревью слияния решений 413 × 414 (релиз 19.11).
//! Н-1 — на стенде с читаемым балансом (tests/t198_live: сервер по формату официальной документации XT, справочник пар —
//! живая запись 06.11, стакан — пример документации, баланс задаёт тест): остановленная сделка с оценкой в итоге дня
//! (решения 53, 152) и остатком «не присвоено»; круг сверки (credit_rest_park::round) решает остаток — количество
//! растёт, оценка позиции в итоге дня пересчитана (daily::provisional — новая заменила прежнюю, одна строка в книге
//! дня), итог дня и пары выросли; соучастник совместного решения — тоже.
//! Н-2 — чистая функция drawdown_fact::wallet_usdt: у кошелька DEX нет строки USDT — «нет данных» (Err, решение 140),
//! строка есть — свободно + заморожено.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core; tests/t198_common, tests/t198_live.

mod t198_common;
mod t198_live;

use arb::core::Core;
use arb::core::credit_fact::{Basis, confirm, rest, set_rest};
use arb::core::deal::{Deal, Step};
use arb::core::types::{Balance, VenueId};
use t198_common::{credited, d};
use t198_live::{big, start};

/// Остановленная сделка BTC на xt: запись 1, подтверждено `qty`, остаток `r`; оценка в итоге дня по `qty` — как
/// ledger::finish для остановленной (daily::provisional), затем сохранена в учёт.
async fn halted(core: &Core, id: &str, qty: &str, r: &str) -> Deal {
    let mut a = credited(id, "xt", d("1"));
    a.direction.asset.ticker = "BTC".into();
    confirm(&mut a, d(qty), Basis::Balance);
    set_rest(&mut a, d(r));
    a.step = Step::Halted;
    arb::core::daily::provisional(core, &mut a).await;
    arb::core::ledger::save(core, &a).await;
    a
}

/// Сделка из учёта памяти.
fn mem(core: &Core, id: &str) -> Deal {
    core.inner.deals.lock().unwrap().get(id).cloned().unwrap()
}

/// Сколько строк «оценка позиции» у сделки в начислениях дня.
fn estimates(x: &Deal) -> usize {
    x.day_book
        .iter()
        .filter(|e| e.what == "оценка позиции")
        .count()
}

/// Н-1: круг сверки решил остаток остановленной сделки (0.2 + 0.3, на счёте 1) — количество 0.5, оценка в итоге дня
/// пересчитана по 0.5 (по рынку — стакан стенда), итог дня и пары выросли; прежняя оценка заменена, не задвоена.
#[test]
fn round_revalues_halted_deal() {
    big(async {
        let (core, _) = start(&t198_common::dir("merge-round"), "1").await;
        let a = halted(&core, "V1", "0.2", "0.3").await;
        let old = a.provisional_usd.expect("оценка до решения");
        let day0 = core.control().day;
        let pair = day0.deal_pairs.get("V1").cloned();
        let pair0 = pair.as_ref().map(|p| day0.pairs[p].provisional_usd);
        arb::core::credit_rest_park::round(&core, &a.at).await;
        let x = mem(&core, "V1");
        assert_eq!((x.qty, rest(&x)), (d("0.5"), None), "остаток решён");
        let day = core.control().day;
        assert!(
            !day.unpriced.contains("V1"),
            "оценка по рынку (стакан стенда)"
        );
        let new = x.provisional_usd.expect("оценка после решения");
        assert!(new > old, "оценка выросла: {old} → {new}");
        assert_eq!(day.provisional["V1"].0, new, "в итоге дня — новая оценка");
        assert_eq!(
            day.provisional_usd - day0.provisional_usd,
            new - old,
            "итог дня: прежняя снята, новая добавлена"
        );
        if let (Some(p), Some(p0)) = (pair, pair0) {
            assert!(day.pairs[&p].provisional_usd > p0, "итог пары вырос");
        }
        assert_eq!(estimates(&x), 2, "одна оценка до решения и одна после");
        assert!(
            x.timeline
                .iter()
                .filter(|(_, t)| t.contains("В итог дня"))
                .count()
                >= 2,
            "строка пересчёта в хронологии"
        );
    });
}

/// Н-1, соучастник: две остановленные с остатком решаются совместно одним кругом — оценки обеих пересчитаны.
#[test]
fn round_revalues_joint_peer() {
    big(async {
        let (core, _) = start(&t198_common::dir("merge-joint"), "1").await;
        let a = halted(&core, "J1", "0.2", "0.3").await;
        let b = halted(&core, "J2", "0.1", "0.2").await;
        let (oa, ob) = (a.provisional_usd.unwrap(), b.provisional_usd.unwrap());
        arb::core::credit_rest_park::round(&core, &a.at).await;
        let (x, y) = (mem(&core, "J1"), mem(&core, "J2"));
        assert_eq!((rest(&x), rest(&y)), (None, None), "обе решены");
        assert!(x.qty > d("0.2") && y.qty > d("0.1"), "количества выросли");
        assert!(x.provisional_usd.unwrap() > oa, "оценка J1 выросла");
        assert!(y.provisional_usd.unwrap() > ob, "оценка J2 выросла");
        let day = core.control().day;
        assert_eq!(day.provisional["J1"].0, x.provisional_usd.unwrap());
        assert_eq!(day.provisional["J2"].0, y.provisional_usd.unwrap());
    });
}

/// Н-2: строки USDT у кошелька DEX нет — «нет данных» (Err), не 0; строка есть — свободно + заморожено; строка 0 — 0.
#[test]
fn dex_wallet_without_usdt_row_is_no_data() {
    let v = VenueId("bsc_dex".into());
    let e = arb::core::drawdown_fact::wallet_usdt(&v, None, "USDT").unwrap_err();
    assert!(e.contains("нет данных о USDT кошелька"), "{e}");
    let row = |free: &str, locked: &str| Balance {
        venue: v.clone(),
        asset: "USDT".into(),
        free: d(free),
        locked: d(locked),
    };
    let ok = arb::core::drawdown_fact::wallet_usdt(&v, Some(&row("12.5", "0")), "USDT");
    assert_eq!(ok, Ok(d("12.5")));
    let zero = arb::core::drawdown_fact::wallet_usdt(&v, Some(&row("0", "0")), "USDT");
    assert_eq!(zero, Ok(d("0")));
}
