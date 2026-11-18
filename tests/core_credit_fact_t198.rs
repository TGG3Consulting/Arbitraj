//! Файл: tests/core_credit_fact_t198.rs
//! Что делает: решение 413 (t198) — «фактическое количество монет сделки на площадке» на записанных числах живого
//! случая WKC → MEXC 18.11.2026 (tests/data/wkc_mexc_2026-11-18/journal.json) и на настоящем ядре с SQLite во временной
//! папке (площадок в реестре нет, моков нет; факт счёта передаётся тем же путём, что после чтения баланса —
//! remain_fact::apply_fact):
//! - чистое распределение credit_split::split — исчерпывающе: обе сделки ждут доли (факт сверки 21:38:19), по очереди
//!   (как шло вживую), биржа зачислила больше, посторонние до вывода, пусто, в пути (unclear), учёт подтверждённых больше
//!   факта, независимость от порядка, допуск, замер однозначен / нет;
//! - ожидаемое удержание (deposit_hold::expected_of) и строка затрат DepositHold;
//! - ядро: вторая сделка WKC больше не видит «своих монет нет» (было — алерт), учёт площадки (сверка) после долей не
//!   больше факта — паузы нет; ждущая доли в сверке не участвует; своя доля 0 — алерт, сделка не закрыта; баланс не
//!   читается — оценка, не запись; «мало средств» на продаже — повтор с фактом один раз (sell_fact::wanted,
//!   credit_fact::mark_reread).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::gateway; данные — tests/data.

use std::path::PathBuf;

use arb::core::Core;
use arb::core::credit_fact::{Basis, CreditFact};
use arb::core::credit_split::{Input, Pending, split};
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{Asset, CostKind, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::errors::ErrorCode;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Число из строки. Ошибок нет (тест падает).
fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Записанный журнал живого случая.
fn journal() -> Value {
    let p = format!(
        "{}/tests/data/wkc_mexc_2026-11-18/journal.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Число журнала по пути JSON Pointer.
fn j(v: &Value, ptr: &str) -> Decimal {
    d(v.pointer(ptr).and_then(Value::as_str).unwrap())
}

/// Ждущая доли.
fn pend(id: &str, record: Decimal, outsider: Option<Decimal>) -> Pending {
    Pending {
        id: id.into(),
        record,
        outsider,
    }
}

/// Входы без удержания и допуска.
fn input(fact: Decimal, confirmed: Decimal, pending: Vec<Pending>) -> Input {
    Input {
        fact,
        confirmed,
        pending,
        unclear: false,
        hold_pct: Decimal::ZERO,
        tol_pct: Decimal::ZERO,
    }
}

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
    assert!(sp.measurable);
    assert!(sp.over, "ожидалось 0 % — удержано больше ожидаемого");
    assert_eq!(sp.excess, Decimal::ZERO);
}

/// Как шло вживую: первая сделка зачислена одна (на счёте ровно 98 % её записи), подтверждена; затем вторая — её доля
/// = факт сверки минус подтверждённая доля первой (с хвостом ~0.67 монеты), не больше её записи; «своих монет нет» нет.
#[test]
fn wkc_sequential_second_deal_sees_own_coins() {
    let w = journal();
    let (r1, r2) = (
        j(&w, "/deals/0/deposit_record"),
        j(&w, "/deals/1/deposit_record"),
    );
    let fact = j(&w, "/reconcile/on_venue");
    let first_fact = r1 * d("0.98");
    let sp1 = split(&input(first_fact, Decimal::ZERO, vec![pend("A", r1, None)]));
    let s1 = sp1.share_of("A").unwrap();
    assert_eq!(s1, d("5887892665.5528"));
    assert!(!sp1.measurable, "посторонние не сняты — замер неоднозначен");
    let sp2 = split(&input(fact, s1, vec![pend("B", r2, None)]));
    let s2 = sp2.share_of("B").unwrap();
    assert_eq!(s2, d("27390513.5940"));
    assert!(s2 < r2 && s2 > Decimal::ZERO);
    assert!(
        s1 + s2 <= fact,
        "учёт после долей не больше факта — сверка без расхождения"
    );
    // Было (решение 405): вторая вычитала ЗАПИСЬ первой — на счёте для неё ничего («своих монет не видно»).
    assert!(fact - r1 < Decimal::ZERO);
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

/// Посторонние монеты на счёте до вывода (ручные, старые) — ни одной сделке; берётся наибольший снимок.
#[test]
fn outsiders_are_never_given_to_deals() {
    let sp = split(&input(
        d("1098"),
        Decimal::ZERO,
        vec![
            pend("A", d("1000"), Some(d("100"))),
            pend("B", d("100"), Some(d("90"))),
        ],
    ));
    assert_eq!(sp.outsider, d("100"));
    assert_eq!(sp.pool, d("998"));
    // Точность долей — точность входных чисел (здесь целые): вниз до целого, остаток округления никому.
    assert_eq!(sp.share_of("A"), Some(d("907")));
    assert_eq!(sp.share_of("B"), Some(d("90")));
    assert!(sp.share_of("A").unwrap() + sp.share_of("B").unwrap() <= sp.pool);
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

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-t198-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе во временной папке; mexc, bingx — биржи, bsc_dex — DEX.
async fn start(name: &str) -> Core {
    let d = dir(name);
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (VenueId("mexc".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    core
}

/// Сделка WKC DEX → `to`, зачисленная на `to` записью `record`, ждёт доли (шаг «продажа»).
fn credited(id: &str, to: &str, record: Decimal) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "WKC".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId(to.into()),
    };
    let mut x = Deal::new(id.into(), dir, json!({}), d("18.22"));
    x.step = Step::Selling;
    x.at = VenueId(to.into());
    x.qty = record;
    x.invested = d("262.99");
    x.credit = Some(Box::new(CreditFact {
        venue: VenueId(to.into()),
        record,
        outsider: None,
        share: None,
        basis: None,
        at: chrono::Utc::now(),
        reread: None,
    }));
    x
}

/// Живой случай на ядре: первая подтверждается фактом (98 % записи), вторая видит свои монеты (было — алерт «своих
/// монет не видно»), учёт площадки после долей не больше факта сверки (паузы площадки нет), ждущая — вне сверки.
#[tokio::test]
async fn wkc_live_flow_on_core() {
    use arb::core::reconcile_confirm::expected;
    let core = start("wkc").await;
    let w = journal();
    let mexc = VenueId("mexc".into());
    let (r1, r2) = (
        j(&w, "/deals/0/deposit_record"),
        j(&w, "/deals/1/deposit_record"),
    );
    let mut a = credited("20261118213557076WKC", "mexc", r1);
    arb::core::ledger::save(&core, &a).await;
    assert!(
        expected(&core, &mexc, None).is_empty(),
        "ждёт доли — вне сверки"
    );
    assert!(arb::core::remain_fact::apply_fact(&core, &mut a, r1 * d("0.98")).await);
    assert_eq!(a.qty, d("5887892665.5528"));
    let c = a.credit.as_deref().unwrap();
    assert_eq!((c.share, c.basis), (Some(a.qty), Some(Basis::Balance)));
    assert!(
        a.timeline
            .iter()
            .any(|(_, t)| t.contains("удержано биржей"))
    );
    arb::core::ledger::save(&core, &a).await;
    let mut b = credited("20261118213605442WKC", "mexc", r2);
    arb::core::ledger::save(&core, &b).await;
    let fact = j(&w, "/reconcile/on_venue");
    assert!(arb::core::remain_fact::apply_fact(&core, &mut b, fact).await);
    assert_eq!(b.qty, d("27390513.5940"));
    assert_eq!(b.step, Step::Selling, "не алерт человеку");
    arb::core::ledger::save(&core, &b).await;
    let need: Decimal = expected(&core, &mexc, None).values().map(|e| e.0).sum();
    assert!(
        need <= fact,
        "учёт {need} ≤ факт {fact}: расхождения сверки нет"
    );
    // Подтверждённая первая на остатке: её учёт минус ничего (вторая подтверждена) — без урезания.
    assert!(arb::core::remain_fact::apply_fact(&core, &mut a, fact).await);
    assert_eq!(a.qty, d("5887892665.5528"));
}

/// Своя доля 0 (запись есть, на счёте пусто) — алерт человеку, сделка не закрыта, учёт не тронут.
#[tokio::test]
async fn pending_with_empty_account_goes_to_human() {
    let core = start("empty").await;
    let mut a = credited("E1", "mexc", d("100"));
    arb::core::ledger::save(&core, &a).await;
    assert!(!arb::core::remain_fact::apply_fact(&core, &mut a, Decimal::ZERO).await);
    let rec = core.inner.deals.lock().unwrap().get("E1").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman);
    assert_eq!(rec.qty, d("100"));
}

/// Баланс не читается — продаётся оценка «запись × (1 − ожидаемое удержание)» (замер 2 %), не запись; основание —
/// Estimate.
#[tokio::test]
async fn unreadable_balance_sells_estimate() {
    let core = start("estimate").await;
    let mut a = credited("S1", "bingx", d("1000"));
    arb::core::deposit_hold::put(arb::core::deposit_hold::Hold {
        venue: VenueId("bingx".into()),
        asset: a.direction.asset.clone(),
        records: d("1000"),
        credited: d("980"),
        pct: d("2"),
        at: chrono::Utc::now(),
        deals: vec!["X".into()],
    });
    arb::core::credit_apply::estimate(&core, &mut a, "лимит запросов").await;
    assert_eq!(a.qty, d("980"));
    assert_eq!(a.credit.as_deref().unwrap().basis, Some(Basis::Estimate));
}

/// «Мало средств» на продаже биржи: повтор с фактом — один раз; подтверждённое записью снова ждёт доли.
#[tokio::test]
async fn insufficient_balance_rereads_once() {
    use arb::core::op_error::OpError;
    let core = start("short").await;
    let mut a = credited("I1", "mexc", d("100"));
    arb::core::credit_fact::confirm(&mut a, d("100"), Basis::Record);
    assert!(!arb::core::credit_fact::pending(&a));
    let e = OpError {
        code: Some(ErrorCode::InsufficientBalance),
        message: "MEXC 30005 Oversold".into(),
        sent: false,
        cancelled: false,
        price_moved: false,
    };
    assert!(arb::core::sell_fact::wanted(&core, &a, &e));
    arb::core::credit_fact::mark_reread(&mut a, d("100"));
    assert!(
        arb::core::credit_fact::pending(&a),
        "запись — снова ждёт доли"
    );
    assert!(
        !arb::core::sell_fact::wanted(&core, &a, &e),
        "второй отказ — прежний путь"
    );
    arb::core::credit_fact::clear_reread(&mut a);
    assert!(arb::core::sell_fact::wanted(&core, &a, &e));
    // DEX и чужой код — не сюда.
    let mut x = a.clone();
    x.at = VenueId("bsc_dex".into());
    assert!(!arb::core::sell_fact::wanted(&core, &x, &e));
    let other = OpError {
        code: Some(ErrorCode::OrderNotFilled),
        ..e
    };
    assert!(!arb::core::sell_fact::wanted(&core, &a, &other));
}
