//! Проверки штатного входа IOC, общей денежной брони и холодной сверки старого клиентского ID.
use super::*;
use crate::{
    core::{
        deps::Deps,
        types::{Asset, VenueId},
    },
    gateway::registry::Registry,
    notify::dispatch::Notifier,
    settings::{scope::Scope, store::Store},
    storage::{db::Db, journal::Journal, ledger_db::count_io},
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};
use tokio::sync::broadcast;
#[path = "cash_order_test_venue.rs"]
mod simulated;
use simulated::Spot;
fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "arb-cash-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}
async fn start(p: &Path) -> Core {
    let db = Db::open(&p.join("test.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    settings
        .set(
            "venue.quote_asset",
            Scope::Venue("bingx".into()),
            Some(json!("USDT")),
            "test",
        )
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: crate::secrets::Vault::locked(p.join("secrets.enc")),
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
        data_dir: p.to_owned(),
    })
    .await
}
fn buy(id: &str, cap: i64) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bingx".into()),
        asset: Asset {
            chain: None,
            contract: None,
            ticker: "X".into(),
        },
        side: Side::Buy,
        qty: cap.into(),
        worst_price: Decimal::ONE,
        deal_id: id.into(),
        route: None,
        exit_position: false,
        remeasure: None,
    }
}
async fn mixed(core: &Core, spot: Arc<Spot>, req: TradeRequest) -> GatewayResult<TradeResult> {
    intent::scope(
        &core.inner.deps.db,
        "mixed-buy",
        intent::Request::Order(Box::new(req.clone())),
        order(core, spot, req, 1, true),
    )
    .await
}
#[tokio::test]
async fn ordinary_inactive_buy_has_zero_added_sql_and_balance_requests() {
    let p = dir();
    let core = start(&p).await;
    let spot = Arc::new(Spot::default());
    let (result, io) = count_io(order(&core, spot.clone(), buy("legacy", 80), 1, false)).await;
    result.unwrap();
    assert_eq!(io, (0, 0));
    assert_eq!(spot.reads.load(Ordering::SeqCst), 0);
    assert_eq!(spot.posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn mixed_unknown_then_ordinary_buy_and_cold_replay_use_the_same_account() {
    let p = dir();
    let spot = Arc::new(Spot::default());
    let req = buy("base", 60);
    {
        let core = start(&p).await;
        *spot.unknown.lock().unwrap() = true;
        assert_eq!(
            mixed(&core, spot.clone(), req.clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::OutcomeUnknown
        );
        assert_eq!(
            order(&core, spot.clone(), buy("bsc", 80), 1, false)
                .await
                .unwrap_err()
                .code,
            ErrorCode::InsufficientBalance
        );
    }
    let core = start(&p).await;
    assert_eq!(
        order(&core, spot.clone(), buy("bsc", 80), 1, false)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InsufficientBalance
    );
    *spot.account.lock().unwrap() = "changed".into();
    assert_eq!(
        mixed(&core, spot.clone(), req.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::OutcomeUnknown
    );
    *spot.account.lock().unwrap() = "original-uid".into();
    *spot.unknown.lock().unwrap() = false;
    let fill = mixed(&core, spot.clone(), req).await.unwrap();
    assert_eq!(fill.filled_qty, 30.into());
    assert_eq!(spot.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        order(&core, spot.clone(), buy("bsc", 80), 1, false)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InsufficientBalance
    );
    let bucket = spot.bucket();
    *spot.balance.lock().unwrap() = 68.into();
    spot.witnessed.lock().unwrap().push(funds::Booked {
        bucket: bucket.clone(),
        order: fill.venue_ref.clone(),
        debit: 32.into(),
    });
    order(&core, spot.clone(), buy("bsc", 68), 1, false)
        .await
        .unwrap();
    assert_eq!(spot.posts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn ordinary_then_mixed_adopts_the_original_in_flight_order_without_a_second_post() {
    let p = dir();
    let core = start(&p).await;
    let enter = Arc::new(tokio::sync::Notify::new());
    let leave = Arc::new(tokio::sync::Notify::new());
    let venue = Arc::new(Spot {
        enter: Some(enter.clone()),
        leave: Some(leave.clone()),
        ..Spot::default()
    });
    *venue.unknown.lock().unwrap() = true;
    let regular = tokio::spawn({
        let c = core.clone();
        let v = venue.clone();
        async move { order(&c, v, buy("bsc", 80), 1, false).await }
    });
    enter.notified().await;
    let other = buy("base", 80);
    assert_eq!(
        mixed(&core, venue.clone(), other.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::InsufficientBalance
    );
    leave.notify_one();
    assert_eq!(
        regular.await.unwrap().unwrap_err().code,
        ErrorCode::OutcomeUnknown
    );
    assert_eq!(venue.posts.load(Ordering::SeqCst), 1);
    drop(core);
    let core = start(&p).await;
    assert_eq!(
        mixed(&core, venue.clone(), other).await.unwrap_err().code,
        ErrorCode::InsufficientBalance
    );
    *venue.unknown.lock().unwrap() = false;
    order(&core, venue.clone(), buy("bsc", 80), 1, false)
        .await
        .unwrap();
    assert_eq!(venue.posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn interrupted_before_receiving_the_native_id_preserves_only_the_saved_budget_on_restart() {
    use crate::core::{
        deal::{Deal, Step},
        types::Direction,
    };
    let p = dir();
    {
        let core = start(&p).await;
        let req = buy("ordinary", 80);
        let mut d = Deal::new(
            req.deal_id,
            Direction {
                asset: req.asset,
                buy_on: req.venue,
                sell_on: VenueId("dex".into()),
            },
            json!({"size":{"budget":"80"},"cash_buy_origin":{"owner":"signature-owner","currency":"USDT","cap":"80"}}),
            Decimal::ZERO,
        );
        d.step = Step::Halted;
        crate::core::persist::save_deal(&core.inner.deps.db, &d)
            .await
            .unwrap();
    }
    let core = start(&p).await;
    let venue = Arc::new(Spot::default());
    assert_eq!(
        mixed(&core, venue.clone(), buy("base", 21))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InsufficientBalance
    );
    *venue.unknown.lock().unwrap() = true;
    assert_eq!(
        mixed(&core, venue.clone(), buy("base", 20))
            .await
            .unwrap_err()
            .code,
        ErrorCode::OutcomeUnknown
    );
    assert_eq!(venue.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn terminal_ordinary_fill_before_first_mixed_is_saved_with_its_exact_debit_and_client_id() {
    use crate::core::{
        deal::{Deal, Step},
        types::Direction,
    };
    let p = dir();
    let venue = Arc::new(Spot::default());
    {
        let core = start(&p).await;
        let req = buy("ordinary-first", 80);
        let mut d = Deal::new(
            req.deal_id.clone(),
            Direction {
                asset: req.asset.clone(),
                buy_on: req.venue.clone(),
                sell_on: VenueId("dex".into()),
            },
            json!({}),
            Decimal::ZERO,
        );
        d.buy = Some(order(&core, venue.clone(), req, 1, false).await.unwrap());
        d.step = Step::Closed;
        remember(&core, &mut d);
        assert_eq!(
            d.plan["cash_legacy_ioc"][0]["lookup"]["client_id"],
            "client-ordinary-first"
        );
        crate::core::persist::save_deal(&core.inner.deps.db, &d)
            .await
            .unwrap();
        let (answer, io) =
            count_io(order(&core, venue.clone(), buy("unrelated", 80), 1, false)).await;
        answer.unwrap();
        assert_eq!(io, (0, 0));
    }
    let core = start(&p).await;
    assert_eq!(
        mixed(&core, venue.clone(), buy("shared", 69))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InsufficientBalance
    );
    let uid = venue.bucket();
    crate::storage::cex_funds_original::observed(
        core.inner.deps.db.ledger(),
        &uid,
        "exchange-ordinary-first",
        32.into(),
        json!({"source":"synthetic-native-event"}),
    )
    .await
    .unwrap();
    let (answer, io) = count_io(order(
        &core,
        venue.clone(),
        buy("after-disable", 80),
        1,
        false,
    ))
    .await;
    answer.unwrap();
    assert_eq!(io, (0, 0));
    let before = venue.posts.load(Ordering::SeqCst);
    order(&core, venue.clone(), buy("ordinary-first", 80), 1, false)
        .await
        .unwrap();
    assert_eq!(venue.posts.load(Ordering::SeqCst), before);
}
