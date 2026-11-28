//! Файл: core/settlement_to_trading_tests.rs
//! Что делает: проверки шага «зачисленное — на торговый счёт» круга Base (settlement_to_trading.rs): первый проход,
//! повтор после перезапуска (отметки started / ready / failed), сбой перевода с остановкой до ордера, биржа без
//! перевода (Ok(None)) и отметка в сохранённом денежном пути.
//! Кто вызывает: cargo test.
//! От чего зависит: test_core.rs (ядро на временной базе), settlement_to_trading_test_venue.rs (тестовая биржа
//! фонд/спот того же вида, что cash_order_test_venue.rs).
use super::deal::Deal;
use super::settlement_state::{Leg, Path, Plan};
use super::settlement_to_trading::{FIELD, Store, ensure};
use super::types::{Asset, DexAccountId, Direction, Transfer, TransferStatus, VenueId};
use rust_decimal::Decimal;
use std::sync::{Arc, atomic::Ordering};

#[path = "settlement_to_trading_test_venue.rs"]
mod venue;
use venue::Funds;

/// Ядро на временной базе с тестовой биржей. Принимает: биржу. Возвращает: ядро.
async fn core(v: Arc<Funds>) -> super::Core {
    let dir = std::env::temp_dir().join(format!(
        "arb-to-trading-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let core = super::test_core::open(&dir).await;
    core.inner.deps.gw.register(v).await;
    core
}

fn asset() -> Asset {
    Asset {
        chain: None,
        contract: None,
        ticker: "X".into(),
    }
}

fn credit(amount: i64) -> Transfer {
    Transfer {
        venue: VenueId("bingx".into()),
        asset: asset(),
        network: "BASE".into(),
        amount: amount.into(),
        status: TransferStatus::Credited,
        venue_ref: "dep-1".into(),
        tx_hash: Some("0xabc".into()),
        fee: None,
        updated_at: chrono::Utc::now(),
    }
}

fn direction() -> Direction {
    Direction {
        asset: asset(),
        buy_on: VenueId("dex".into()),
        sell_on: VenueId("bingx".into()),
    }
}

fn deal() -> Deal {
    Deal::new(
        "t-to-trading".into(),
        direction(),
        serde_json::json!({}),
        Decimal::ZERO,
    )
}

/// Прогон шага для сделки. Возвращает: итог ensure.
async fn run(core: &super::Core, d: &mut Deal, c: &Transfer) -> Result<(), String> {
    let (dex, cex) = (VenueId("dex".into()), VenueId("bingx".into()));
    ensure(core, Store::Deal(d), (&dex, &cex), &asset(), "X", c).await
}

fn state(d: &Deal) -> Option<String> {
    d.plan
        .get(FIELD)
        .and_then(|v| v.get("state"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

#[tokio::test]
async fn first_pass_moves_the_credit_to_trading() {
    let v = Arc::new(Funds::new(10, 0, false));
    let core = core(v.clone()).await;
    let mut d = deal();
    run(&core, &mut d, &credit(10)).await.unwrap();
    assert_eq!(v.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*v.spot.lock().unwrap(), Decimal::from(10));
    assert_eq!(state(&d).as_deref(), Some("ready"));
    // Повтор после записи «ready»: ни перевода, ни чтения баланса.
    run(&core, &mut d, &credit(10)).await.unwrap();
    assert_eq!(v.calls.load(Ordering::SeqCst), 1);
    assert_eq!(v.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn restart_after_started_with_coin_on_trading_does_not_move_again() {
    let v = Arc::new(Funds::new(0, 10, false));
    let core = core(v.clone()).await;
    let mut d = deal();
    d.plan[FIELD] = serde_json::json!({"tx": "0xabc", "state": "started"});
    run(&core, &mut d, &credit(10)).await.unwrap();
    assert_eq!(v.calls.load(Ordering::SeqCst), 0);
    assert_eq!(state(&d).as_deref(), Some("ready"));
}

#[tokio::test]
async fn failed_move_stops_before_the_order_and_waits_for_manual_transfer() {
    let v = Arc::new(Funds::new(0, 0, false));
    let core = core(v.clone()).await;
    let mut d = deal();
    let e = run(&core, &mut d, &credit(10)).await.unwrap_err();
    assert!(e.contains("ордер не ставится"), "{e}");
    assert_eq!(state(&d).as_deref(), Some("failed"));
    let tried = v.calls.load(Ordering::SeqCst);
    // Повтор: нового перевода нет; человек перевёл вручную — шаг закрыт.
    run(&core, &mut d, &credit(10)).await.unwrap_err();
    assert_eq!(v.calls.load(Ordering::SeqCst), tried);
    *v.spot.lock().unwrap() = 10.into();
    run(&core, &mut d, &credit(10)).await.unwrap();
    assert_eq!(v.calls.load(Ordering::SeqCst), tried);
    assert_eq!(state(&d).as_deref(), Some("ready"));
}

#[tokio::test]
async fn venue_without_move_keeps_the_old_path() {
    let v = Arc::new(Funds::new(0, 10, true));
    let core = core(v.clone()).await;
    let mut d = deal();
    run(&core, &mut d, &credit(10)).await.unwrap();
    assert_eq!(v.calls.load(Ordering::SeqCst), 1);
    assert_eq!(v.reads.load(Ordering::SeqCst), 0);
    assert_eq!(state(&d).as_deref(), Some("ready"));
}

#[tokio::test]
async fn return_path_keeps_the_mark_in_the_saved_path() {
    let v = Arc::new(Funds::new(10, 0, false));
    let core = core(v.clone()).await;
    let db = core.inner.deps.db.clone();
    let plan = Plan {
        deal: "t-return".into(),
        direction: direction(),
        cex: VenueId("bingx".into()),
        wallet: DexAccountId("w".into()),
        leg: Leg::Return {
            available_amount: 10.into(),
            minimum_price: Decimal::ONE,
        },
    };
    let mut path = Path {
        market: None,
        plan: plan.clone(),
        conversion: None,
        movement: None,
        to_trading: None,
    };
    super::settlement_recovery::save(&db, &path, None)
        .await
        .unwrap();
    let (dex, cex) = (VenueId("dex".into()), VenueId("bingx".into()));
    let c = credit(10);
    ensure(
        &core,
        Store::Path(&db, &mut path),
        (&dex, &cex),
        &asset(),
        "X",
        &c,
    )
    .await
    .unwrap();
    let saved = super::settlement_recovery::get(&db, &plan)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.to_trading, path.to_trading);
    assert_eq!(saved.to_trading.unwrap()["state"], "ready");
    assert_eq!(v.calls.load(Ordering::SeqCst), 1);
}
