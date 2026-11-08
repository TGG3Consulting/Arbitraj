//! Файл: tests/htx_points_fee.rs
//! Что проверяет: комиссия HTX баллами HB (решение 232, дополнение; ветка t42/small-fixes).
//! - Разбор matchresults (venues/htx/order_fee.rs): fee-deduct-currency «hbpoint», filled-points — часть HBPOINT со
//!   стоимостью в USDT 1:1 (Fee.usd) по справке HTX «Point Card Usage Instructions»: «1 pts = 1 USDT fee»; HT — как
//!   раньше (живая цена, Fee.usd нет). Строки собраны по полям документации «Get the Match Result of an Order»
//!   (живых ордеров HTX в журнале бота нет — как tests/fees_232.rs).
//! - Учёт сделки на живом ядре (core/fills.rs, SQLite во временной папке, моков нет): часть со стоимостью в USDT
//!   идёт в затраты как есть, количество монеты не уменьшает, хронология — «… HBPOINT (= … USDT)», пометки «не
//!   пересчитана» нет.
//! - Запись сделки: старая часть без usd читается (None), новая пишется с usd и читается обратно.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::htx::order_fee, arb::core (fills, deal), arb::settings, arb::storage, arb::notify.

use std::str::FromStr;

use arb::core::Core;
use arb::core::deal::Deal;
use arb::core::deps::Deps;
use arb::core::types::{Asset, Direction, Fee, TradeResult, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::htx::order_fee;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// matchresults: две сделки списаны баллами HB, одна — HT, одна — в монете: HBPOINT суммируется вместе со стоимостью.
#[test]
fn hbpoint_part_has_usdt_value() {
    let data = json!([
        {"filled-amount": "100", "filled-fees": "0", "fee-currency": "cake",
         "fee-deduct-currency": "hbpoint", "filled-points": "0.0123", "fee-deduct-state": "done"},
        {"filled-amount": "40", "filled-fees": "0", "fee-currency": "cake",
         "fee-deduct-currency": "hbpoint", "filled-points": "0.0077", "fee-deduct-state": "done"},
        {"filled-amount": "10", "filled-fees": "0", "fee-currency": "cake",
         "fee-deduct-currency": "ht", "filled-points": "0.001", "fee-deduct-state": "done"},
        {"filled-amount": "50", "filled-fees": "0.1", "fee-currency": "cake",
         "fee-deduct-currency": "", "filled-points": "0"}
    ]);
    let fees = order_fee::parse(&data, "CAKE").unwrap();
    assert_eq!(
        fees,
        vec![
            Fee {
                asset: "HBPOINT".into(),
                amount: d("0.0200"),
                usd: Some(d("0.0200")),
            },
            Fee {
                asset: "HT".into(),
                amount: d("0.001"),
                usd: None,
            },
            Fee {
                asset: "CAKE".into(),
                amount: d("0.1"),
                usd: None,
            },
        ]
    );
    assert_eq!(
        Fee::text(&fees),
        "0.0200 HBPOINT (= 0.0200 USDT) + 0.001 HT + 0.1 CAKE"
    );
}

/// Старая запись части комиссии (без usd) читается; новая — с usd, без usd поле не пишется.
#[test]
fn fee_record_compat() {
    let old: Fee = serde_json::from_value(json!({"asset": "USDT", "amount": "0.1"})).unwrap();
    assert_eq!(old.usd, None);
    let v = serde_json::to_value(&old).unwrap();
    assert!(v.get("usd").is_none(), "{v}");
    let p = Fee {
        asset: "HBPOINT".into(),
        amount: d("0.05"),
        usd: Some(d("0.05")),
    };
    let back: Fee = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    assert_eq!(back, p);
}

/// Ядро на базе во временной папке (без площадок в реестре).
async fn start(name: &str) -> Core {
    let dir = std::env::temp_dir().join(format!("arb-htxpts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
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
        data_dir: dir.clone(),
    })
    .await
}

/// Покупка на HTX с комиссией баллами: затраты += стоимость баллов, количество — всё исполненное, пометки нет.
#[tokio::test]
async fn ledger_takes_points_at_doc_rate() {
    let core = start("ledger").await;
    let dir = Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("htx".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut deal = Deal::new("pts1".into(), dir, json!(null), Decimal::ZERO);
    let req = json!({"venue": "htx", "asset": {"ticker": "CAKE", "bsc_contract": null}, "side": "Buy",
        "qty": "10", "worst_price": "1.9", "deal_id": "pts1", "route": null});
    let r: TradeResult = serde_json::from_value(json!({"request": req, "status": "Filled",
        "filled_qty": "10", "avg_price": "1.89",
        "fees": [{"asset": "HBPOINT", "amount": "0.0378", "usd": "0.0378"}],
        "venue_ref": "1", "chain": null, "finished_at": "2026-11-08T10:00:00Z"}))
    .unwrap();
    arb::core::fills::trade(&core, &mut deal, &r).await;
    assert_eq!(deal.qty, d("10"));
    assert_eq!(deal.spent, d("0.0378"));
    assert!(
        deal.timeline
            .iter()
            .any(|(_, t)| t.contains("комиссия 0.0378 HBPOINT (= 0.0378 USDT)")),
        "{:?}",
        deal.timeline
    );
    assert!(
        !deal
            .timeline
            .iter()
            .any(|(_, t)| t.contains("не пересчитана")),
        "{:?}",
        deal.timeline
    );
}
