//! Файл: tests/core_credit_fact_t198.rs
//! Что делает: решение 413 (t198) — «фактическое количество монет сделки на площадке» на настоящем ядре с SQLite во
//! временной папке (площадок в реестре нет, моков нет; факт счёта передаётся тем же путём, что после чтения баланса —
//! remain_fact::apply_fact) на записанных числах живого случая WKC → MEXC 18.11 (tests/data/wkc_mexc_2026-11-18):
//! вторая сделка WKC больше не видит «своих монет нет», учёт площадки после долей не больше факта (паузы нет), ждущая
//! — в сверке нижней оценкой; своя доля 0 — алерт, сделка не закрыта; баланс биржи не прочитан — не продаём: режим
//! застрявшей монеты один раз, потом алерт (повторное ревью, Б-3); «мало средств» — повтор с фактом один раз; любое
//! зачисление на биржу ждёт доли (С1); вывод с биржи — после факта: executor (первый вывод), «Перегон», «Продать и
//! перегнать» (по исходникам без комментариев). Чистое распределение — tests/core_credit_split_t198.rs.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::gateway; tests/t198_common.

mod t198_common;

use std::path::PathBuf;

use arb::core::Core;
use arb::core::credit_fact::{Basis, CreditFact};
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::errors::ErrorCode;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::json;
use t198_common::{d, j, journal};
use tokio::sync::broadcast;

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
        outsider_at: None,
        share: None,
        basis: None,
        at: chrono::Utc::now(),
        reread: None,
    }));
    x
}

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

/// Повторное ревью (Б-3): баланс биржи не прочитан — не продаём вслепую: первый раз режим застрявшей монеты (здесь
/// stuck.mode не задан — сделка остановлена с причиной), второй раз подряд — алерт человеку.
#[tokio::test]
async fn unread_balance_never_sells_blind() {
    let core = start("unread").await;
    let mut a = credited("U1", "mexc", d("100"));
    arb::core::ledger::save(&core, &a).await;
    assert!(!arb::core::credit_alert::unread(&core, &mut a, "лимит запросов").await);
    assert!(a.fact_unread);
    let rec = core.inner.deals.lock().unwrap().get("U1").cloned().unwrap();
    assert_eq!(
        rec.step,
        Step::Halted,
        "режим застрявшей монеты без stuck.mode — остановка с причиной"
    );
    assert_eq!(rec.qty, d("100"), "не продано");
    let mut b = credited("U2", "mexc", d("100"));
    b.fact_unread = true;
    arb::core::ledger::save(&core, &b).await;
    assert!(!arb::core::credit_alert::unread(&core, &mut b, "лимит запросов").await);
    let rec = core.inner.deals.lock().unwrap().get("U2").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman, "повторно — человеку");
}

/// Код без строк-комментариев (`//`). Ошибок нет.
fn code_of(f: &str) -> String {
    let p = format!("{}/{f}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Б2 и повторное ревью Б-2: в коде (без комментариев) каждый вывод монеты сделки (transfers::send_trade) и оценка
/// перегона (relay_value::value_at) идут после доли по факту (sell_fact::before) — executor (первый вывод),
/// «Перегон» (relay.rs), «Продать и перегнать» (manual_relay.rs: run → relay_to).
#[test]
fn withdraw_paths_read_fact_first() {
    for f in [
        "src/core/executor.rs",
        "src/core/relay.rs",
        "src/core/manual_relay.rs",
    ] {
        let src = code_of(f);
        let fact = src
            .find("sell_fact::before(")
            .unwrap_or_else(|| panic!("{f}: нет факта"));
        for what in ["transfers::send_trade(", "relay_value::value_at("] {
            for (at, _) in src.match_indices(what) {
                assert!(fact < at, "{f}: {what} раньше факта");
            }
        }
    }
    // «Продать и перегнать»: relay_to зовётся только из run после факта.
    let m = code_of("src/core/manual_relay.rs");
    let fact = m.find("sell_fact::before(").unwrap();
    for (at, _) in m.match_indices("relay_to(core") {
        assert!(fact < at);
    }
}
