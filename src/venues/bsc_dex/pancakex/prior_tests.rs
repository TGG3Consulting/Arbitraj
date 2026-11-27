//! О-49. Настройки, повтор обычным маршрутом и восстановление после открытия реального файла учёта.
use super::{ledger, lifecycle_tests::marker, market, recovery::prior};
use crate::{
    core::types::{DexRoute, RouteMode, Side, TradeRequest},
    gateway::{errors::ErrorCode, settlement_intent as intent},
    settings::scope::Scope,
    storage::ledger_db::count_io,
    venues::bsc_dex::{
        hashflow::recovery_tests::{directory, parts},
        parts::Parts,
    },
};
use alloy_primitives::B256;
use rust_decimal::Decimal;
use serde_json::{Value, json};

async fn set(p: &Parts, key: &str, value: Value) {
    p.ctx
        .settings
        .set(
            key,
            Scope::Venue(p.ctx.place.id.to_string()),
            Some(value),
            "test",
        )
        .await
        .unwrap();
}
fn request(p: &Parts, m: &ledger::Marker) -> TradeRequest {
    TradeRequest {
        venue: p.ctx.venue(),
        asset: crate::core::types::Asset {
            chain: None,
            contract: Some(m.ticket.req.input),
            ticker: "GOOGLc".into(),
        },
        side: Side::Sell,
        qty: Decimal::ONE,
        worst_price: Decimal::ONE,
        deal_id: "order-49".into(),
        route: None,
        exit_position: false,
        remeasure: None,
    }
}
fn relay() -> DexRoute {
    DexRoute {
        mode: RouteMode::Relay(B256::repeat_byte(1)),
        legs: vec![],
        block: 0,
        gas_units: 0,
        executable: true,
        exec_note: Some(market::LABEL.into()),
    }
}

#[tokio::test]
async fn disabled_channel_and_foreign_contracts_never_touch_sqlite() {
    let m = marker();
    let dir = directory("pancakex-prior", 56);
    let p = parts(&dir, 56, m.ticket.req.owner).await;
    set(&p, "dex.evm_chain_id", json!(56)).await;
    let profiles: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/pancakex_profiles.json"
    ))
    .unwrap();
    set(
        &p,
        "pancakex.contracts",
        profiles["base"]["contracts"].clone(),
    )
    .await;
    for enabled in [false, true] {
        set(&p, "pancakex.trading", json!(enabled)).await;
        let mut r = request(&p, &m);
        for route in [None, Some(relay())] {
            r.route = route;
            let (found, io) = count_io(prior(&p, &r)).await;
            assert!(found.unwrap().is_none());
            assert_eq!(io, (0, 0));
        }
    }
}

#[tokio::test]
async fn disabled_channel_keeps_the_previous_signature_after_restart() {
    let mut m = marker();
    let owner = m.ticket.req.owner;
    let dir = directory("pancakex-restart", 8453);
    {
        let p = parts(&dir, 8453, owner).await;
        set(&p, "dex.evm_chain_id", json!(8453)).await;
        let profiles: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/pancakex_profiles.json"
        ))
        .unwrap();
        set(
            &p,
            "pancakex.contracts",
            profiles["base"]["contracts"].clone(),
        )
        .await;
        set(&p, "pancakex.trading", json!(false)).await;
        let mut disabled = request(&p, &m);
        disabled.route = Some(relay());
        let (result, io) = count_io(prior(&p, &disabled)).await;
        assert!(result.unwrap().is_none());
        assert_eq!(io, (0, 0));
        set(&p, "pancakex.trading", json!(true)).await;
        let mut r = request(&p, &m);
        m.account = p.ctx.account.clone().unwrap();
        m.key = ledger::identity(&p.ctx, &r).unwrap();
        let db = p.ctx.journal.db();
        let writing = db.ledger().pancakex_orders.begin(8453, &m.key).unwrap();
        // Конкурентная попытка не может увидеть ещё не записанное поручение как свободное.
        assert_eq!(
            prior(&p, &r).await.unwrap_err().code,
            ErrorCode::OutcomeUnknown
        );
        intent::scope(
            db,
            &m.key,
            intent::Request::Order(Box::new(r.clone())),
            intent::before_send(&m.pending()),
        )
        .await
        .unwrap();
        writing.recorded();
        assert!(m.reserve(db, &m.terms().unwrap()).await.unwrap());
        set(&p, "pancakex.trading", json!(false)).await;
        set(&p, "pancakex.contracts", json!([])).await;
        // Новый объём с тем же идентификатором обязан вернуть маркер исходного заказа.
        r.qty += Decimal::ONE;
        assert_eq!(
            prior(&p, &r).await.unwrap_err().pending.as_deref(),
            Some(&m.pending())
        );
    }
    let p = parts(&dir, 8453, owner).await;
    let mut r = request(&p, &m);
    r.qty += Decimal::ONE;
    assert_eq!(
        prior(&p, &r).await.unwrap_err().pending.as_deref(),
        Some(&m.pending())
    );
    r.deal_id.push_str("-other");
    let (found, io) = count_io(prior(&p, &r)).await;
    assert!(found.unwrap().is_none());
    assert_eq!(io, (0, 0));
}

#[tokio::test]
async fn damaged_permit_isolated_from_bsc_and_new_signatures() {
    let mut m = marker();
    let dir = directory("pancakex-damaged", 8453);
    {
        let p = parts(&dir, 8453, m.ticket.req.owner).await;
        set(&p, "dex.evm_chain_id", json!(8453)).await;
        let r = request(&p, &m);
        m.key = ledger::identity(&p.ctx, &r).unwrap();
        m.account = p.ctx.account.clone().unwrap();
        let db = p.ctx.journal.db();
        intent::scope(
            db,
            &m.key,
            intent::Request::Order(Box::new(r)),
            intent::before_send(&m.pending()),
        )
        .await
        .unwrap();
        assert!(m.reserve(db, &m.terms().unwrap()).await.unwrap());
        db.ledger()
            .write(|c| {
                c.execute(
                    "UPDATE core_state SET body='damaged' WHERE kind='pancakex_permit'",
                    [],
                )
            })
            .await
            .unwrap();
    }
    {
        let p = parts(&dir, 8453, m.ticket.req.owner).await;
        let unresolved = prior(&p, &request(&p, &m)).await.unwrap_err();
        assert_eq!(unresolved.code, ErrorCode::OutcomeUnknown);
        assert_eq!(unresolved.pending.as_deref(), Some(&m.pending()));
    }
    let p = parts(&dir, 56, m.ticket.req.owner).await;
    set(&p, "dex.evm_chain_id", json!(56)).await;
    let (result, io) = count_io(prior(&p, &request(&p, &m))).await;
    assert!(result.unwrap().is_none());
    assert_eq!(io, (0, 0));
    let index = &p.ctx.journal.db().ledger().pancakex_orders;
    assert!(index.begin(8453, "new").is_none());
    assert!(index.begin(1, "other").is_some());
    drop(p);
    let offline = crate::storage::db::Db::open_existing(&dir.join("state.sqlite"), false).unwrap();
    assert!(
        offline
            .ledger()
            .pancakex_orders
            .begin(8453, "new")
            .is_none()
    );
}

#[tokio::test]
async fn ordinary_trade_future_excludes_rpc_and_receipt_state() {
    // Измеряем конкретные типы без конструирования или опроса сетевых операций.
    fn bytes<F: Future>(_: impl FnOnce() -> F) -> usize {
        std::mem::size_of::<F>()
    }

    let m = marker();
    let dir = directory("pancakex-size", 8453);
    let p = parts(&dir, 8453, m.ticket.req.owner).await;
    let r = request(&p, &m);
    let terms = m.terms().unwrap();
    let pending = m.pending();
    let h = crate::venues::bsc_dex::quote::white_state::Head {
        hash: m.anchor,
        number: alloy_primitives::U64::from(m.start),
        timestamp: alloy_primitives::U64::ZERO,
    };
    let hot = bytes(|| prior(&p, &r));
    let recovery = bytes(|| super::recovery::reconcile(&p, r.clone(), &pending));
    let scan = bytes(|| super::history::scan(&p.ctx, &m, &terms, h));
    let paid = bytes(|| super::result::paid(&p, r.clone(), &m, &terms, m.anchor));
    let profile = bytes(|| m.ticket.cfg.p.check(&p.ctx, h, &m.ticket.offer.version));
    let sizes =
        json!({"prior":hot, "reconcile":recovery,"history":scan,"receipt":paid,"roles":profile});
    if let Some(path) = std::env::var_os("ARB_PANCAKEX_FUTURE_SIZES") {
        std::fs::write(path, sizes.to_string()).unwrap();
    }
    assert!(hot < 1024 && hot < scan && hot < paid, "{sizes}");
}
