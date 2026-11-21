//! Файл: tests/core_credit_fact_t198.rs
//! Что делает: решение 413 (t198) — «фактическое количество монет сделки на площадке» на настоящем ядре с SQLite во
//! временной папке (площадок в реестре нет, моков нет; факт счёта передаётся тем же путём, что после чтения баланса —
//! remain_fact::apply_fact) на записанных числах живого случая WKC → MEXC 18.11 (tests/data/wkc_mexc_2026-11-18):
//! вторая сделка WKC больше не видит «своих монет нет», учёт площадки после долей не больше факта (паузы нет), ждущая
//! — в сверке нижней оценкой; своя доля 0 — алерт, сделка не закрыта; «мало средств» — повтор с фактом один раз;
//! любое зачисление на биржу ждёт доли (С1). Баланс не прочитан, вывод после факта и правки третьего ревью —
//! tests/core_credit_review3_t198.rs. Чистое распределение — tests/core_credit_split_t198.rs,
//! tests/core_credit_split2_t198.rs.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::gateway; tests/t198_common.

mod t198_common;

use arb::core::credit_fact::Basis;
use arb::core::deal::Step;
use arb::core::types::VenueId;
use arb::gateway::errors::ErrorCode;
use rust_decimal::Decimal;
use t198_common::{credited, d, j, journal, start};

/// Живой случай на ядре: первая подтверждается фактом (98 % записи), вторая видит свои монеты (было — алерт «своих
/// монет не видно»), учёт площадки после долей не больше факта сверки (паузы площадки нет), ждущая — в сверке нижней оценкой.
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
    // Ревью t198: ждущая доли — в сверке нижней оценкой запись × (1 − ожидаемое удержание; здесь не измерено — 0).
    let low: Decimal = expected(&core, &mexc, None).values().map(|e| e.0).sum();
    assert_eq!(low, r1);
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

/// С1: любое зачисление на биржу ждёт доли (и у монет без налога); на DEX — записи нет; факт перед продажей на бирже —
/// всегда.
#[tokio::test]
async fn every_cex_credit_waits_for_share() {
    let core = start("c1").await;
    let mut a = credited("C1", "mexc", d("100"));
    a.credit = None;
    arb::core::credit_fact::on_credit(&core, &mut a, &VenueId("mexc".into()), Some("0xabc"));
    assert!(arb::core::credit_fact::pending(&a));
    assert!(arb::core::sell_fact::need(&core, &a));
    let c = a.credit.as_deref().unwrap();
    assert_eq!((c.record, c.outsider), (d("100"), None), "снимка нет — Б3");
    let mut x = credited("C2", "bsc_dex", d("100"));
    arb::core::credit_fact::on_credit(&core, &mut x, &VenueId("bsc_dex".into()), None);
    assert!(x.credit.is_none());
    assert!(!arb::core::sell_fact::need(&core, &x));
}

/// Сверка (ревью t198): ждущая доли — нижней оценкой запись × (1 − ожидаемое удержание) (замер 2 %).
#[tokio::test]
async fn reconcile_counts_pending_by_lower_estimate() {
    let core = start("low").await;
    let mut a = credited("L1", "mexc", d("1000"));
    a.direction.asset.ticker = "LOWEST".into();
    let s = arb::core::deposit_hold::Sample {
        records: d("1000"),
        credited: d("980"),
        pct: d("2"),
        at: chrono::Utc::now(),
        deals: vec!["L0".into()],
    };
    arb::core::deposit_hold::put(&core, &VenueId("mexc".into()), &a.direction.asset, s);
    arb::core::ledger::save(&core, &a).await;
    let m = arb::core::reconcile_confirm::expected(&core, &VenueId("mexc".into()), None);
    let low: Decimal = m.values().map(|e| e.0).sum();
    assert_eq!(low, d("980"));
}
