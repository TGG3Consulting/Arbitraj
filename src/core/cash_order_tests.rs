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
        remember(&core, &mut d);
        crate::core::ledger::finish(&core, &mut d, Step::Closed, "синтетическая проверка").await;
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

#[tokio::test]
async fn confirmed_other_currency_with_an_old_key_does_not_charge_this_bucket() {
    use crate::core::{deal::Deal, types::Direction};
    use crate::storage::cex_funds_watch::Prior;
    let p = dir();
    let core = start(&p).await;
    let venue = Arc::new(Spot::default());
    let q = buy("other-currency", 80);
    let mut d = Deal::new(
        q.deal_id.clone(),
        Direction {
            asset: q.asset.clone(),
            buy_on: q.venue.clone(),
            sell_on: VenueId("dex".into()),
        },
        json!({}),
        Decimal::ZERO,
    );
    let foreign = Prior {
        action: "archived-foreign".into(),
        request: q,
        owner: "retired-credential".into(),
        lookup: PendingRef::order("X-USDC", "original-foreign", 80.into()),
        currency: Some("USDC".into()),
        limit: None,
        fill: None,
    };
    d.plan["cash_legacy_ioc"] = json!([foreign]);
    crate::core::persist::save_deal(&core.inner.deps.db, &d)
        .await
        .unwrap();
    mixed(&core, venue.clone(), buy("base", 80)).await.unwrap();
    assert_eq!(venue.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn protected_unknown_poll_uses_only_original_native_account_and_original_client() {
    let core = start(&dir()).await;
    let venue = Arc::new(Spot::default());
    let req = buy("lost", 80);
    *venue.unknown.lock().unwrap() = true;
    let err = mixed(&core, venue.clone(), req.clone()).await.unwrap_err();
    let p = err.pending.unwrap();
    let key = p.cash_action.as_deref().unwrap();
    assert!(
        resolve(&core, venue.as_ref(), &req, &p, key)
            .await
            .unwrap()
            .is_none()
    );
    *venue.unknown.lock().unwrap() = false;
    *venue.account.lock().unwrap() = "different-uid".into();
    assert_eq!(
        resolve(&core, venue.as_ref(), &req, &p, key)
            .await
            .unwrap_err()
            .code,
        ErrorCode::OutcomeUnknown
    );
    *venue.account.lock().unwrap() = "original-uid".into();
    let fill = resolve(&core, venue.as_ref(), &req, &p, key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fill.venue_ref, "exchange-lost");
    assert_eq!(venue.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_known_local_failure_before_dispatch_releases_only_the_owned_original() {
    let core = start(&dir()).await;
    let venue = Arc::new(Spot::default());
    *venue.reject_before_wire.lock().unwrap() = true;
    let q = buy("failed-before-send", 80);
    assert_eq!(
        mixed(&core, venue.clone(), q.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotConfigured
    );
    assert!(!core.inner.deps.db.ledger().cash_watch.unresolved("bingx"));
    assert!(
        intent::load(&core.inner.deps.db, "mixed-buy")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(venue.posts.load(Ordering::SeqCst), 0);
    *venue.reject_before_wire.lock().unwrap() = false;
    mixed(&core, venue.clone(), q).await.unwrap();
    assert_eq!(venue.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prepared_kucoin_fee_at_the_bsc_budget_boundary_preserves_the_ordinary_order() {
    use crate::core::{deal::Deal, types::Direction};
    let core = start(&dir()).await;
    core.inner
        .deps
        .settings
        .set(
            "venue.quote_asset",
            Scope::Venue("kucoin".into()),
            Some(json!("USDT")),
            "test",
        )
        .await
        .unwrap();
    let venue = Arc::new(Spot {
        id: "kucoin",
        ..Spot::default()
    });
    core.inner.deps.gw.register(venue.clone()).await;
    let book: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/data/cex_samples_2026_11_04/kucoin_orderbook_level2_20_CAKE-USDT.json"
    ))
    .unwrap();
    let symbols: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/data/cex_samples_2026_11_04/kucoin_symbols_v2.json"
    ))
    .unwrap();
    let market = crate::venues::kucoin::symbols::parse_list(&symbols["data"], "USDT")
        .unwrap()
        .0;
    let worst = serde_json::from_value::<Decimal>(book["data"]["asks"][1][0].clone()).unwrap();
    let mut req = buy("bsc-budget", 10);
    req.venue = venue.id();
    req.asset.ticker = "CAKE".into();
    (req.qty, req.worst_price) = crate::venues::kucoin::order_rules::check_rules(
        &market["CAKE"],
        Side::Buy,
        req.qty,
        worst,
        "IOC",
    )
    .unwrap();
    *venue.fee_rate.lock().unwrap() = Decimal::new(1, 3);
    let budget = Decimal::new(1896895, 5);
    let mut deal = Deal::new(
        req.deal_id.clone(),
        Direction {
            asset: req.asset.clone(),
            buy_on: req.venue.clone(),
            sell_on: VenueId("bsc_dex".into()),
        },
        json!({"size":{"budget":budget}}),
        Decimal::ZERO,
    );
    opening(&core, &mut deal, req.qty, req.worst_price, budget).await;
    assert_eq!(
        serde_json::from_value::<Decimal>(deal.plan[OPENING]["cap"].clone()).unwrap(),
        budget
    );
    crate::core::ledger::save(&core, &deal).await;
    let (result, io) = count_io(order(&core, venue.clone(), req.clone(), 1, false)).await;
    result.unwrap();
    assert_eq!(io, (0, 0));
    assert_eq!(venue.reads.load(Ordering::SeqCst), 0);
    // Тот же исходный бюджет, настоящая смена таксы на границе между планом и prepared POST.
    req.deal_id = "bsc-rate-changed".into();
    deal.id = req.deal_id.clone();
    crate::core::ledger::save(&core, &deal).await;
    *venue.fee_rate.lock().unwrap() = Decimal::new(2, 3);
    let (result, io) = count_io(order(&core, venue.clone(), req.clone(), 1, false)).await;
    result.unwrap();
    assert_eq!(io, (0, 1));
    assert_eq!(venue.reads.load(Ordering::SeqCst), 0);
    let keys = super::cold::unadopted(core.inner.deps.db.ledger(), &req.venue)
        .await
        .unwrap();
    assert_eq!(
        keys[&cash_key(&req, 1, None).unwrap()]
            .limit
            .as_ref()
            .unwrap()
            .1,
        Decimal::new(189879, 4)
    );
    assert_eq!(venue.posts.load(Ordering::SeqCst), 2);
}
