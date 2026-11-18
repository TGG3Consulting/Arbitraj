//! Файл: tests/core_credit_split_t198.rs
//! Что делает: решение 413 (t198) — чистое распределение факта счёта биржи между сделками, ждущими доли
//! (credit_split::split), на записанных числах живого случая WKC → MEXC 18.11 (tests/data/wkc_mexc_2026-11-18): обе
//! ждут, по очереди как вживую, лишнее записи, посторонние — самый свежий снимок, пусто, в пути, независимость от
//! порядка. Края, допуск, ожидаемое удержание, снимок и мера — tests/core_credit_split2_t198.rs. Моков нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::credit_split, tests/t198_common.

mod t198_common;

use arb::core::credit_split::split;
use rust_decimal::Decimal;
use serde_json::Value;
use t198_common::{d, input, j, journal, pend};
/// Обе сделки WKC ждут доли, факт — сверка 21:38:19: доли пропорционально записям, вместе не больше факта, удержано
/// ровно ~2 % (налог токена), каждой — меньше её записи; замер однозначен (посторонние сняты).
#[test]
fn wkc_both_pending_split_by_records() {
    let w = journal();
    let (r1, r2) = (
        j(&w, "/deals/0/deposit_record"),
        j(&w, "/deals/1/deposit_record"),
    );
    let fact = j(&w, "/reconcile/on_venue");
    assert_eq!(
        r1 + r2,
        j(&w, "/reconcile/ledger"),
        "учёт сверки = сумма записей"
    );
    let id1 = w.pointer("/deals/0/id").and_then(Value::as_str).unwrap();
    let id2 = w.pointer("/deals/1/id").and_then(Value::as_str).unwrap();
    let z = Some(Decimal::ZERO);
    let sp = split(&input(
        fact,
        Decimal::ZERO,
        vec![pend(id1, r1, z), pend(id2, r2, z)],
    ));
    assert_eq!(sp.share_of(id1), Some(d("5887892666.2232")));
    assert_eq!(sp.share_of(id2), Some(d("27390512.9235")));
    let given = sp.share_of(id1).unwrap() + sp.share_of(id2).unwrap();
    assert!(given <= fact);
    assert_eq!(sp.held, d("120720064.1933"));
    assert!(sp.held_pct() > d("1.9999") && sp.held_pct() < d("2"));
    assert!(
        !sp.measurable,
        "две ждущие — замер неоднозначен (ревью t198, С2)"
    );
    assert!(sp.over, "ожидалось 0 % — удержано больше ожидаемого");
    assert_eq!(sp.excess, Decimal::ZERO);
}

/// Как шло вживую, на записанных числах (повторное ревью t198): первая зачислена, вторая ещё в пути через счёт —
/// доля первой не выше записи × (1 − налог перевода токена 2 %, journal.json token_transfer_tax_pct) при факте сверки;
/// затем вторая после подтверждённой первой — пул (факт − доля первой) больше её записи × 0.98: доля = запись × 0.98,
/// хвост 0.6736 — «не присвоено». «Своих монет нет» у второй больше нет (было — вычитала ЗАПИСЬ первой).
#[test]
fn wkc_sequential_second_deal_sees_own_coins() {
    let w = journal();
    let (r1, r2) = (
        j(&w, "/deals/0/deposit_record"),
        j(&w, "/deals/1/deposit_record"),
    );
    let (fact, tax) = (
        j(&w, "/reconcile/on_venue"),
        j(&w, "/token_transfer_tax_pct"),
    );
    let mut i1 = input(fact, Decimal::ZERO, vec![pend("A", r1, None)]);
    i1.unclear = true;
    i1.hold_pct = tax;
    let sp1 = split(&i1);
    let s1 = sp1.share_of("A").unwrap();
    assert_eq!(s1, d("5887892665.5528"));
    assert!(!sp1.measurable, "в пути и без снимка — замер неоднозначен");
    let mut i2 = input(fact, s1, vec![pend("B", r2, None)]);
    i2.hold_pct = tax;
    let sp2 = split(&i2);
    let s2 = sp2.share_of("B").unwrap();
    assert_eq!(s2, d("27390512.9204"));
    assert_eq!(sp2.unassigned, d("0.6736"));
    assert!(
        s1 + s2 <= fact,
        "учёт после долей не больше факта — сверка без расхождения"
    );
    assert!(
        fact - r1 < Decimal::ZERO,
        "было: факт − запись первой < 0 — «своих монет нет»"
    );
}

/// Биржа зачислила больше записи (round-up, airdrop) — каждой ровно её запись, лишнее никому.
#[test]
fn credited_more_than_record_is_not_assigned() {
    let sp = split(&input(
        d("105"),
        Decimal::ZERO,
        vec![pend("A", d("100"), Some(d("0")))],
    ));
    assert_eq!(sp.share_of("A"), Some(d("100")));
    assert_eq!(sp.excess, d("5"));
    assert_eq!(sp.held, Decimal::ZERO);
    assert_eq!(sp.held_pct(), Decimal::ZERO);
}

/// Посторонние монеты на счёте до вывода (ручные, старые) — ни одной сделке; берётся САМЫЙ СВЕЖИЙ снимок (повторное
/// ревью, С-3).
#[test]
fn outsiders_are_never_given_to_deals() {
    let (t1, t2) = (
        chrono::Utc::now() - chrono::Duration::seconds(10),
        chrono::Utc::now(),
    );
    let mut a = pend("A", d("1000"), Some(d("100")));
    let mut b = pend("B", d("100"), Some(d("90")));
    a.seen = Some(t2);
    b.seen = Some(t1);
    let sp = split(&input(d("1098"), Decimal::ZERO, vec![a.clone(), b.clone()]));
    assert_eq!(sp.outsider, d("100"));
    assert_eq!(sp.pool, d("998"));
    // Точность долей — точность входных чисел (здесь целые): вниз до целого, остаток округления никому.
    assert_eq!(sp.share_of("A"), Some(d("907")));
    assert_eq!(sp.share_of("B"), Some(d("90")));
    assert!(sp.share_of("A").unwrap() + sp.share_of("B").unwrap() <= sp.pool);
    // Свежее снят снимок B — берётся его 90.
    a.seen = Some(t1);
    b.seen = Some(t2);
    assert_eq!(
        split(&input(d("1098"), Decimal::ZERO, vec![a, b])).outsider,
        d("90")
    );
}

/// На счёте ничего для ждущих (или подтверждённые съели весь факт) — всем 0, удержано всё.
#[test]
fn empty_pool_gives_zero() {
    let sp = split(&input(
        d("50"),
        d("60"),
        vec![pend("A", d("10"), Some(d("0")))],
    ));
    assert_eq!(sp.share_of("A"), Some(Decimal::ZERO));
    assert_eq!(sp.held, d("10"));
    let sp = split(&input(
        Decimal::ZERO,
        Decimal::ZERO,
        vec![pend("A", d("10"), None)],
    ));
    assert_eq!(sp.share_of("A"), Some(Decimal::ZERO));
}

/// Через счёт идёт сделка в пути (её монеты могли уже прийти) — доля не выше записи × (1 − ожидаемое удержание).
#[test]
fn unclear_caps_by_expected_hold() {
    let mut i = input(
        d("1000"),
        Decimal::ZERO,
        vec![pend("A", d("100"), Some(d("0")))],
    );
    i.unclear = true;
    i.hold_pct = d("2");
    let sp = split(&i);
    assert_eq!(sp.share_of("A"), Some(d("98")));
    assert!(!sp.measurable, "в пути — замер неоднозначен");
}

/// Доли не зависят от порядка ждущих; без ждущих — ничего.
#[test]
fn order_independent_and_empty() {
    let a = vec![
        pend("A", d("30"), Some(d("0"))),
        pend("B", d("70"), Some(d("0"))),
    ];
    let mut b = a.clone();
    b.reverse();
    let (x, y) = (
        split(&input(d("98.00"), d("0"), a)),
        split(&input(d("98.00"), d("0"), b)),
    );
    for id in ["A", "B"] {
        assert_eq!(x.share_of(id), y.share_of(id));
    }
    assert_eq!(x.share_of("A"), Some(d("29.4")));
    let e = split(&input(d("5"), d("0"), vec![]));
    assert!(e.shares.is_empty() && e.held.is_zero() && !e.measurable);
}
