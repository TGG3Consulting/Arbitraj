//! Файл: tests/core_credit_split_t198.rs
//! Что делает: решение 413 (t198) — чистое распределение факта счёта биржи между сделками, ждущими доли
//! (credit_split::split), на записанных числах живого случая WKC → MEXC 18.11 (tests/data/wkc_mexc_2026-11-18): обе
//! ждут, по очереди как вживую, края (лишнее — хвост сверки 0.6736, пусто, в пути, посторонние — самый свежий снимок,
//! без снимка, со снимком и удержанием — ручные монеты после снимка не уходят, допуск округления и замер 0 %),
//! независимость от порядка, переполнение; ожидаемое удержание и строка DepositHold; снимок посторонних при сделке в
//! пути / занятом замке — нет; мера — наибольший из последних N замеров. Моков нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{credit_split, credit_base, deposit_hold, remain_others}, tests/t198_common.

mod t198_common;

use arb::core::credit_split::split;
use arb::core::types::CostKind;
use rust_decimal::Decimal;
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

/// Б1: снимок посторонних при сделке в пути через счёт (или при занятом замке) — None, никогда не «посторонние»;
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
    o.busy = true;
    assert_eq!(
        outsider_of(d("1"), &o, d("2")),
        None,
        "замок не взят — снимка нет"
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
