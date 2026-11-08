//! Файл: tests/core_outcome_resolve.rs
//! Что проверяет: решение 231 — бот сам выясняет исход «исход неизвестен» (чистая логика, без ядра и сети):
//! 1) «чем найти» (gateway/pending.rs) прикладывается только к OutcomeUnknown, внутренний (с ID ордера) не
//!    перезаписывается внешним; ключ — свой ID, иначе ID площадки; пусто — ключа нет;
//! 2) учёт открытых операций площадки (core/outcome_open.rs): пауза Outcome снимается, только когда выяснены ВСЕ
//!    операции с ключом; операция без ключа или не выясненная за срок — «только человек», пауза остаётся;
//! 3) пауза, стоявшая до «исход неизвестен» (человек, правило), при снятии возвращается как была; пауза сверки прежней не
//!    считается; пауза по убытку дня поверх — остаётся;
//! 4) «Снять паузу» человеком (учёт снят) — бот больше не выясняет и паузу не трогает (Gone);
//! 5) найденный вывод (pending::created) — перевод Pending с ID площадки и полями запроса.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{auto_pause, outcome_open, risk}, arb::gateway::{errors, pending}.

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;

use arb::core::auto_pause::{self as ap, Kind, VenueMark};
use arb::core::outcome_open::{self as open, Lift};
use arb::core::risk::Risk;
use arb::core::types::{Asset, TransferStatus, VenueId, WithdrawRequest};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::gateway::pending::{self, PendingRef};

fn mexc() -> VenueId {
    VenueId("mexc".into())
}

/// «Исход неизвестен» на площадке так же, как rules_stop::outcome: прежняя пауза → пауза Outcome → учёт.
fn outcome(r: &mut Risk, v: &VenueId, key: Option<&str>) {
    let prior = open::prior(r, v);
    let now = Utc.with_ymd_and_hms(2026, 11, 8, 12, 0, 0).unwrap();
    ap::pause_venue(r, v, "Покупка: исход неизвестен".into(), Kind::Outcome, now);
    open::register(r, v, key, prior);
}

#[test]
fn pending_only_on_outcome_unknown_and_key() {
    let p = PendingRef::order("CAKEUSDT", "b123456789_20261108", Decimal::new(15, 0));
    let rejected = GatewayError::new(ErrorCode::OrderRejected, None, "отказ").pending(p.clone());
    assert!(
        rejected.pending.is_none(),
        "исход известен — «чем найти» не нужно"
    );
    let inner = p.with_ref("C02__1");
    let e = GatewayError::new(ErrorCode::OutcomeUnknown, None, "нет ответа")
        .pending(inner.clone())
        .pending(p.clone());
    assert_eq!(
        e.pending.as_deref(),
        Some(&inner),
        "внутренний с ID ордера остаётся"
    );
    assert_eq!(
        inner.key(),
        "b123456789_20261108",
        "ключ — свой ID, он не меняется"
    );
    let dex = PendingRef {
        venue_ref: Some("0xabc".into()),
        nonce: Some(7),
        ..PendingRef::default()
    };
    assert_eq!(dex.key(), "0xabc");
    assert_eq!(PendingRef::default().key(), "");
}

#[test]
fn lifted_only_when_all_resolved() {
    let mut r = Risk::default();
    let v = mexc();
    outcome(&mut r, &v, Some("a"));
    outcome(&mut r, &v, Some("b"));
    assert!(open::resolving(&r, &v, "a") && open::resolving(&r, &v, "b"));
    assert_eq!(
        open::resolved(&mut r, &v, "a"),
        Lift::Kept {
            left: 1,
            manual: false
        }
    );
    assert!(
        r.venue_paused.contains_key(&v),
        "ещё одна операция не выяснена"
    );
    assert_eq!(open::resolved(&mut r, &v, "b"), Lift::Lifted);
    assert!(!r.venue_paused.contains_key(&v));
    assert!(!r.venue_auto.contains_key(&v));
    assert!(!r.outcome_open.contains_key(&v));
}

#[test]
fn manual_operation_keeps_pause() {
    // Операция без ключа (адаптер не приложил «чем найти» / сроки не заданы) — «только человек».
    let mut r = Risk::default();
    let v = mexc();
    outcome(&mut r, &v, Some("a"));
    outcome(&mut r, &v, None);
    assert_eq!(
        open::resolved(&mut r, &v, "a"),
        Lift::Kept {
            left: 0,
            manual: true
        }
    );
    assert!(ap::human_only(&r, &v), "пауза Outcome осталась");
    // Не выяснена за срок — тоже человек.
    let mut r = Risk::default();
    outcome(&mut r, &v, Some("x"));
    open::unresolved(&mut r, &v, "x");
    assert!(!open::resolving(&r, &v, "x"), "бот больше не выясняет");
    assert!(ap::human_only(&r, &v));
}

#[test]
fn prior_pause_restored() {
    // Пауза правила шлюза стояла до «исход неизвестен» — вернётся с её отметкой и причиной.
    let mut r = Risk::default();
    let v = mexc();
    let t = Utc.with_ymd_and_hms(2026, 11, 8, 11, 0, 0).unwrap();
    ap::pause_venue(&mut r, &v, "правило шлюза №4".into(), Kind::Rule, t);
    outcome(&mut r, &v, Some("a"));
    assert_eq!(r.venue_auto[&v].kind, Kind::Outcome);
    assert_eq!(open::resolved(&mut r, &v, "a"), Lift::Lifted);
    assert_eq!(r.venue_paused[&v], "правило шлюза №4");
    assert_eq!(
        r.venue_auto.get(&v),
        Some(&VenueMark {
            kind: Kind::Rule,
            since: t
        })
    );
    // Пауза человека (без отметки) — возвращается без отметки.
    let mut r = Risk::default();
    r.venue_paused.insert(v.clone(), "вручную".into());
    outcome(&mut r, &v, Some("a"));
    assert_eq!(open::resolved(&mut r, &v, "a"), Lift::Lifted);
    assert_eq!(r.venue_paused[&v], "вручную");
    assert!(!r.venue_auto.contains_key(&v));
}

#[test]
fn reconcile_not_prior_and_loss_kept() {
    // Пауза сверки (решение 221) прежней не считается: её отметку сняла пауза Outcome, сверка поставит снова.
    let mut r = Risk::default();
    let v = mexc();
    r.venue_paused
        .insert(v.clone(), "расхождение при сверке".into());
    r.reconcile_venues.insert(v.clone());
    outcome(&mut r, &v, Some("a"));
    assert_eq!(open::resolved(&mut r, &v, "a"), Lift::Lifted);
    assert!(!r.venue_paused.contains_key(&v));
    // Пауза по убытку дня поставлена поверх — остаётся до нового дня.
    let mut r = Risk::default();
    outcome(&mut r, &v, Some("a"));
    r.loss_paused.insert(v.0.clone());
    assert_eq!(open::resolved(&mut r, &v, "a"), Lift::Lifted);
    assert!(r.venue_paused.contains_key(&v));
    assert!(!r.venue_auto.contains_key(&v), "отметка Outcome снята");
}

#[test]
fn human_resume_stops_bot() {
    let mut r = Risk::default();
    let v = mexc();
    outcome(&mut r, &v, Some("a"));
    // Как risk::resume_venue: пауза, отметка и учёт сняты.
    r.venue_paused.remove(&v);
    r.venue_auto.remove(&v);
    r.outcome_open.remove(&v);
    assert!(!open::resolving(&r, &v, "a"));
    assert_eq!(open::resolved(&mut r, &v, "a"), Lift::Gone);
    assert!(!r.venue_paused.contains_key(&v));
}

#[test]
fn created_transfer_from_request() {
    let req = WithdrawRequest {
        venue: mexc(),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        network: "BSC".into(),
        address: "0x1".into(),
        amount: Decimal::new(125, 1),
        deal_id: "20261108120000000CAKE".into(),
    };
    let t = pending::created(mexc(), req, "w-77".into());
    assert_eq!(t.status, TransferStatus::Pending);
    assert_eq!(t.venue_ref, "w-77");
    assert_eq!(t.amount, Decimal::new(125, 1));
    assert_eq!(t.network, "BSC");
    assert!(t.tx_hash.is_none());
}
