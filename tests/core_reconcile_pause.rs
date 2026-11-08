//! Файл: tests/core_reconcile_pause.rs
//! Что проверяет: решение 221 (Тигран 08.11.2026) — пауза ПЛОЩАДКИ от сверки балансов и её автоснятие
//! (core/reconcile_pause.rs):
//! 1) подтверждённое расхождение ставит паузу только площадке, где оно есть (Risk.venue_paused + отметка сверки),
//!    системный режим не меняется; другие площадки не на паузе;
//! 2) пауза снимается сама после ledger.reconcile_confirm_rounds чистых кругов ПОДРЯД этой площадки; недоступность
//!    ДРУГОЙ площадки не мешает; балансы этой площадки не получены — круг для неё не чистый;
//! 3) пауза, которую снял/поставил человек или другой модуль (отметки сверки нет), сама не снимается; площадка уже на
//!    паузе по другой причине — сверка её не перехватывает;
//! 4) чистая логика счётчика (Lift) и «чистого круга» площадки (is_clean), тексты по площадкам (mismatch_by_venue).
//!    Пример расхождения — случай 08.11: «PORTO по учёту 119.25, на площадке 0».
//!
//! Ядро — на настоящей SQLite во временной папке (площадок в реестре нет, моков нет), как tests/core_decision_177.rs.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (reconcile_pause, reconcile::Found, reconcile_confirm::UNAVAILABLE, risk), settings,
//! storage, notify.

use std::collections::HashSet;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::reconcile::Found;
use arb::core::reconcile_confirm::UNAVAILABLE;
use arb::core::reconcile_pause::{Lift, is_clean, maybe_lift, mismatch_by_venue, pause};
use arb::core::risk::Mode;
use arb::core::types::VenueId;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use tokio::sync::broadcast;

const PORTO: &str = "mexc: PORTO по учёту 119.25, на площадке 0";

/// Ядро на базе во временной папке; ledger.reconcile_confirm_rounds = 2.
async fn start(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-221-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    settings
        .set(
            "ledger.reconcile_confirm_rounds",
            Scope::Global,
            Some(json!(2)),
            "тест",
        )
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await
}

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Расхождение монеты на площадке (ключ «площадка/монета»).
fn coin(venue: &str, text: &str) -> Found {
    (
        "risk.reconcile_mismatch",
        Some(venue.into()),
        text.into(),
        Some(format!("{venue}/PORTO")),
    )
}

/// «Балансы площадки не получены».
fn unavailable(venue: &str) -> Found {
    (
        "risk.reconcile_mismatch",
        Some(venue.into()),
        format!("{venue}: балансы не получены — таймаут"),
        Some(format!("{venue}{UNAVAILABLE}")),
    )
}

fn set(xs: &[&str]) -> HashSet<String> {
    xs.iter().map(|x| x.to_string()).collect()
}

#[tokio::test]
async fn pauses_only_venue_and_lifts_by_own_rounds() {
    let core = start("own").await;
    core.inner.risk.lock().unwrap().mode = Mode::Running;
    pause(&core, &[coin("mexc", PORTO)]).await;
    let r = core.control();
    assert_eq!(r.mode, Mode::Running, "системный режим не трогается");
    assert!(r.venue_paused[&v("mexc")].contains("PORTO по учёту 119.25"));
    assert!(!r.venue_paused.contains_key(&v("bingx")));
    assert!(r.reconcile_venues.contains(&v("mexc")));
    let mut lift = Lift::default();
    let checked = set(&["mexc"]);
    // Круг 1: mexc чистый, bingx недоступна — чужая недоступность не мешает.
    maybe_lift(&core, &mut lift, &checked, &[unavailable("bingx")]).await;
    assert!(core.control().venue_paused.contains_key(&v("mexc")));
    assert_eq!(lift.clean("mexc"), 1);
    // Круг 2: балансы mexc не получены — круг не чистый, счёт сначала.
    maybe_lift(&core, &mut lift, &checked, &[unavailable("mexc")]).await;
    assert_eq!(lift.clean("mexc"), 0);
    // Площадка не сверялась в круге (выключена) — тоже не чистый.
    maybe_lift(&core, &mut lift, &set(&["bingx"]), &[]).await;
    assert_eq!(lift.clean("mexc"), 0);
    // Два чистых подряд — пауза снята, отметка снята.
    maybe_lift(&core, &mut lift, &checked, &[unavailable("bingx")]).await;
    maybe_lift(&core, &mut lift, &checked, &[unavailable("bingx")]).await;
    let r = core.control();
    assert!(!r.venue_paused.contains_key(&v("mexc")));
    assert!(r.reconcile_venues.is_empty());
}

#[tokio::test]
async fn unavailable_balances_pause_venue_and_lift() {
    let core = start("unavail").await;
    pause(&core, &[unavailable("bingx")]).await;
    assert!(core.control().venue_paused.contains_key(&v("bingx")));
    let mut lift = Lift::default();
    for _ in 0..2 {
        maybe_lift(&core, &mut lift, &set(&["bingx"]), &[]).await;
    }
    assert!(!core.control().venue_paused.contains_key(&v("bingx")));
}

#[tokio::test]
async fn manual_resume_or_other_pause_is_not_lifted() {
    let core = start("manual").await;
    pause(&core, &[coin("mexc", PORTO)]).await;
    // Человек снял паузу, затем её поставил другой модуль (ошибки подряд) — сверка её не снимает.
    core.resume_venue(&v("mexc")).await;
    assert!(core.control().reconcile_venues.is_empty());
    core.inner
        .risk
        .lock()
        .unwrap()
        .venue_paused
        .insert(v("mexc"), "3 ошибок подряд — предел 3".into());
    let mut lift = Lift::default();
    for _ in 0..5 {
        maybe_lift(&core, &mut lift, &set(&["mexc"]), &[]).await;
    }
    assert_eq!(
        core.control().venue_paused[&v("mexc")],
        "3 ошибок подряд — предел 3"
    );
    // Площадка уже на паузе не от сверки — сверка паузу не перехватывает и отметку не ставит.
    pause(&core, &[coin("mexc", PORTO)]).await;
    let r = core.control();
    assert_eq!(r.venue_paused[&v("mexc")], "3 ошибок подряд — предел 3");
    assert!(r.reconcile_venues.is_empty());
}

#[test]
fn lift_counts_in_a_row_per_venue() {
    let mut l = Lift::default();
    assert!(!l.round("mexc", true, true, 3));
    assert!(!l.round("mexc", true, true, 3));
    assert!(
        !l.round("bingx", true, false, 3),
        "чужая площадка — свой счёт"
    );
    assert_eq!(l.clean("mexc"), 2);
    assert!(!l.round("mexc", true, false, 3));
    assert_eq!(l.clean("mexc"), 0, "круг с расхождением — счёт сначала");
    assert!(!l.round("mexc", true, true, 3));
    assert!(!l.round("mexc", true, true, 3));
    assert!(
        l.round("mexc", true, true, 3),
        "третий чистый подряд — снять"
    );
    assert_eq!(l.clean("mexc"), 0);
    for need in [0, 1] {
        assert!(
            Lift::default().round("mexc", true, true, need),
            "need = {need}"
        );
    }
    // Не пауза сверки — не копит и не снимает.
    for _ in 0..3 {
        assert!(!l.round("mexc", false, true, 1));
    }
}

#[test]
fn clean_round_per_venue() {
    let checked = set(&["mexc", "bingx"]);
    let bnb: Found = ("risk.low_bnb", Some("mexc".into()), "мало BNB".into(), None);
    assert!(is_clean("mexc", &checked, &[]));
    assert!(
        is_clean("mexc", &checked, &[bnb]),
        "мало BNB — не расхождение"
    );
    assert!(is_clean("mexc", &checked, &[unavailable("bingx")]));
    assert!(!is_clean("mexc", &checked, &[coin("mexc", "x")]));
    assert!(!is_clean("mexc", &checked, &[unavailable("mexc")]));
    assert!(
        !is_clean("bsc_dex", &checked, &[]),
        "не сверялась — не чистый"
    );
}

#[test]
fn texts_grouped_by_venue() {
    let m = mismatch_by_venue(&[coin("mexc", PORTO), coin("mexc", "b"), unavailable("bingx")]);
    assert_eq!(m["mexc"], format!("{PORTO}; b"));
    assert_eq!(m["bingx"], "bingx: балансы не получены — таймаут");
    assert_eq!(m.len(), 2);
}
