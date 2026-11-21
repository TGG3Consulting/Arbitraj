//! Файл: tests/core_credit_split2_t198.rs
//! Что делает: решение 413 (t198; третье ревью, М-4 — вынесено из tests/core_credit_split_t198.rs) — края
//! распределения на записанных числах WKC → MEXC 18.11 (лишнее — хвост сверки 0.6736, пусто, в пути, посторонние, без
//! снимка, допуск округления и замер 0 %), «удержано больше ожидаемого» с допуском, ожидаемое удержание и строка
//! DepositHold, переполнение, снимок посторонних при сделке в пути или уходящей — нет, мера — наибольший из последних N
//! замеров; третье ревью (С-В): при однозначном замере доля по факту выше ожидаемой. Моков нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{credit_split, credit_base, deposit_hold, remain_others}, tests/t198_common.

mod t198_common;

use arb::core::credit_split::split;
use arb::core::types::CostKind;
use rust_decimal::Decimal;
use t198_common::{d, input, j, journal, pend};

/// Удержание не больше ожидаемого с допуском — over = нет; больше — да.
#[test]
fn over_respects_expected_and_tolerance() {
    let mut i = input(
        d("98"),
        Decimal::ZERO,
        vec![pend("A", d("100"), Some(d("0")))],
    );
    i.hold_pct = d("2");
    assert!(!split(&i).over);
    i.fact = d("97.9");
    assert!(split(&i).over);
    i.tol_pct = d("0.2");
    assert!(!split(&i).over);
}

/// Ожидаемое удержание: DEX / нативная — 0; замер сильнее налога; без замера — налог токена; налога нет — 0.
#[test]
fn expected_hold_rules_and_cost_line() {
    use arb::core::deposit_hold::{expected_of, line};
    assert_eq!(
        expected_of(false, false, Some(d("2")), Some(d("2"))),
        Decimal::ZERO
    );
    assert_eq!(expected_of(true, true, None, Some(d("2"))), Decimal::ZERO);
    assert_eq!(
        expected_of(true, false, Some(d("0")), Some(d("2"))),
        Decimal::ZERO
    );
    assert_eq!(expected_of(true, false, None, Some(d("2"))), d("2"));
    assert_eq!(expected_of(true, false, None, None), Decimal::ZERO);
    assert_eq!(expected_of(true, false, Some(d("1.5")), None), d("1.5"));
    let w = journal();
    let arrive = j(&w, "/deals/0/deposit_record");
    let c = line(d("2"), arrive, "WKC").unwrap();
    assert_eq!(c.kind, CostKind::DepositHold);
    assert_eq!(c.amount, d("120161074.8072"));
    assert!(line(Decimal::ZERO, arrive, "WKC").is_none());
}

/// Края распределения на числах WKC (ревью t198): лишнее — хвост сверки (факт − 0.98 × учёт = 0.6736) никому; пустой
/// счёт; в пути — не выше 98 % записи; посторонние = хвост — вторая получает ровно 98 % своей записи; без снимка
/// посторонних и при ожидаемом удержании 2 % ручные монеты не уходят сделке (Б3).
#[test]
fn wkc_edges() {
    let w = journal();
    let (r1, r2) = (
        j(&w, "/deals/0/deposit_record"),
        j(&w, "/deals/1/deposit_record"),
    );
    let fact = j(&w, "/reconcile/on_venue");
    let tail = fact - (r1 + r2) * d("0.98");
    assert_eq!(tail, d("0.6736"));
    let z = Some(Decimal::ZERO);
    // Лишнее: на счёте запись + хвост — сделке запись, хвост никому; замер однозначен (одна ждущая, снимок, без лишнего — нет).
    let sp = split(&input(r1 + tail, Decimal::ZERO, vec![pend("A", r1, z)]));
    assert_eq!((sp.share_of("A"), sp.excess), (Some(r1), tail));
    assert!(!sp.measurable, "есть лишнее — замер неоднозначен");
    // Пусто.
    let sp = split(&input(Decimal::ZERO, Decimal::ZERO, vec![pend("A", r1, z)]));
    assert_eq!((sp.share_of("A"), sp.held), (Some(Decimal::ZERO), r1));
    // В пути: на счёте факт сверки (98 % обеих записей — меньше записи первой, вся пул её по пропорции), но через счёт
    // идёт сделка в пути — доля не выше 98 % записи. «Не присвоено» — монеты, которые ПО ФАКТУ на счёте и по
    // распределению были бы её (не больше записи), но не отданы из-за ограничения: факт − доля = 27 390 513.5940;
    // «удержано» — записи нет на счёте: запись − факт. Запись = доля + не присвоено + удержано — ничего не спрятано,
    // больше записи не отдано.
    let mut i = input(fact, Decimal::ZERO, vec![pend("A", r1, z)]);
    i.unclear = true;
    i.hold_pct = d("2");
    let sp = split(&i);
    let share = d("5887892665.5528");
    assert_eq!(sp.share_of("A"), Some(share));
    assert_eq!(sp.unassigned, fact - share);
    assert_eq!(sp.unassigned, d("27390513.5940"));
    assert_eq!(sp.held, r1 - fact);
    assert_eq!(share + sp.unassigned + sp.held, r1);
    // Посторонние = хвост: вторая после подтверждённой первой — ровно 98 % записи, однозначный замер 2 %.
    let s1 = r1 * d("0.98");
    let sp = split(&input(fact, s1, vec![pend("B", r2, Some(tail))]));
    assert_eq!(sp.share_of("B"), Some(r2 * d("0.98")));
    assert!(sp.measurable);
    assert_eq!(sp.held_pct(), d("2"));
    // Б3: снимка нет, на счёте ещё 1 000 000 ручных монет (факт сверки + 1 000 000 — точность факта биржи), ожидаемое
    // удержание 2 % — сделке не больше 98 % записи.
    let mut i = input(
        fact + d("1000000"),
        Decimal::ZERO,
        vec![pend("A", r1, None)],
    );
    i.hold_pct = d("2");
    let sp = split(&i);
    assert_eq!(sp.share_of("A"), Some(d("5887892665.5528")));
    assert!(sp.unassigned > Decimal::ZERO && !sp.measurable);
    // Повторное ревью (Б-1): и СО снимком (посторонних 0) ручные монеты, пришедшие после снимка, при удержании 2 % не
    // уходят сделке — доля ≤ 98 % записи.
    let mut i = input(fact + d("1000000"), Decimal::ZERO, vec![pend("A", r1, z)]);
    i.hold_pct = d("2");
    assert_eq!(split(&i).share_of("A"), Some(d("5887892665.5528")));
    // Повторное ревью (С-4): лишнее (хвост 0.6736) в пределах допуска округления — замер однозначен, удержание 0 %.
    let mut i = input(r1 + tail, Decimal::ZERO, vec![pend("A", r1, z)]);
    i.tol_pct = d("0.0001");
    let sp = split(&i);
    assert!(sp.measurable);
    assert_eq!(sp.held_pct(), Decimal::ZERO);
}

/// Ревью t198: запись × пул вне Decimal — без переполнения (деление первым).
#[test]
fn part_does_not_overflow() {
    use arb::core::credit_split::part;
    let big = d("50000000000000000000000000000");
    assert_eq!(part(big, big / d("2"), big, 0), big / d("2"));
    assert_eq!(part(d("10"), d("5"), d("20"), 2), d("2.5"));
}

/// Б1: снимок посторонних при сделке в пути через счёт или уходящей с него — None, никогда не «посторонние»;
/// иначе — факт − подтверждённые − ждущие × (1 − удержание).
#[test]
fn outsider_snapshot_rules() {
    use arb::core::credit_base::outsider_of;
    use arb::core::remain_others::Others;
    let w = journal();
    let r1 = j(&w, "/deals/0/deposit_record");
    let mut o = Others {
        pending: vec![pend("A", r1, None)],
        ..Default::default()
    };
    assert_eq!(
        outsider_of(r1 * d("0.98") + d("5"), &o, d("2")),
        Some(d("5"))
    );
    o.unclear = vec!["B".into()];
    assert_eq!(
        outsider_of(d("1"), &o, d("2")),
        None,
        "сделка в пути — снимка нет"
    );
    o.unclear.clear();
    // Повторное ревью (С-1): через счёт уходит сделка (её монеты здесь или уже нет) — снимок неоднозначен.
    o.leaving = vec!["L".into()];
    assert_eq!(
        outsider_of(d("1"), &o, d("2")),
        None,
        "уходит сделка — снимка нет"
    );
}

/// С2: мера — наибольший % последних n замеров; n < 1 — 1; старые выбрасываются.
#[test]
fn hold_measure_is_max_of_last_n() {
    use arb::core::deposit_hold::{Sample, fold};
    let s = |p: &str| Sample {
        records: d("100"),
        credited: d("100") - d(p),
        pct: d(p),
        at: chrono::Utc::now(),
        deals: vec![],
    };
    let (kept, pct) = fold(vec![s("5"), s("2"), s("0")], 2);
    assert_eq!((kept.len(), pct), (2, d("2")));
    let (kept, pct) = fold(vec![s("5"), s("2"), s("0")], 0);
    assert_eq!((kept.len(), pct), (1, Decimal::ZERO));
    let (_, pct) = fold(vec![s("5"), s("2")], 5);
    assert_eq!(pct, d("5"));
}

/// Третье ревью (С-В): однозначный замер (одна ждущая, снимок есть) — доля по факту (raw) без ограничения ожидаемым:
/// ожидалось 2 %, а биржа зачислила всё (WKC: запись первой сделки) — raw = запись, share = 98 %; own() берёт raw.
#[test]
fn measured_share_is_not_capped_by_expected() {
    let r1 = j(&journal(), "/deals/0/deposit_record");
    let mut i = input(r1, Decimal::ZERO, vec![pend("A", r1, Some(Decimal::ZERO))]);
    i.hold_pct = d("2");
    let sp = split(&i);
    assert!(sp.measurable);
    assert_eq!(sp.raw_of("A"), Some(r1));
    assert_eq!(sp.share_of("A"), Some((r1 * d("0.98")).trunc_with_scale(2)));
}
