//! Файл: tests/core_credit_events.rs
//! Что делает: связка «ожидание → событие → закрытие» (решение 76, ревью #66) в настоящем ядре на SQLite:
//! core/credit_watch.rs (ожидание, хеш отправителя), core/credit_event.rs (сверка события), core/transfer_credit.rs
//! (by_event — закрывает ли событие зачисление). События — настоящие переводы USDT из записи квитанций блока 129702236
//! (tests/data/receipts_2026-10-25/responses.json): 58,69 USDT (0x3263…0835) — «наш» перевод, второй перевод того же
//! блока — «чужой» хеш с той же суммой.
//! - хеш отправителя совпал → закрыто;
//! - хеша отправителя ещё нет → не закрыто, только пробуждение; появился и совпал → закрыто;
//! - событие с чужим хешем до хеша отправителя → снято при set_tx (вернулось как внешнее), не закрыто; настоящее
//!   после — закрывает; чужой хеш после хеша отправителя — к ожиданию не прикрепляется;
//! - transfer.credit_by_event = нет → событие прикреплено, но не закрывает.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, tests/receipts_common. Моков нет: площадок в реестре нет —
//! проверяется решение ядра по событию.

mod receipts_common;

use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::{Asset, BalanceChange, BalanceKind, VenueId};
use arb::core::{credit_event, credit_watch, transfer_credit};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use receipts_common::{load, result};

const OUR: &str = "0x3263576c6e5102aa5d25fcfbf93717423687652f2a6dbd04d205523318f20835";
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

/// Ядро на базе во временной папке и хранилище настроек.
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-credit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
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
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings)
}

/// Настройки сверки у DEX: допуск 0 и закрытие по событию.
async fn configure(s: &Store, close: bool) {
    let v = Scope::Venue("bsc_dex".into());
    s.set(
        "transfer.credit_amount_tolerance_pct",
        v.clone(),
        Some(json!("0")),
        "тест",
    )
    .await
    .unwrap();
    s.set("transfer.credit_by_event", v, Some(json!(close)), "тест")
        .await
        .unwrap();
}

/// Хеш другого перевода USDT того же блока (не нашего).
fn foreign() -> String {
    let rec = load("responses.json");
    let receipts = result(&rec, "receipts 129702236");
    receipts
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["transactionHash"].as_str().unwrap().to_string())
        .find(|h| h != OUR)
        .unwrap()
}

fn usdt() -> Asset {
    Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT.parse().unwrap()),
    }
}

/// Поступление 58,69 USDT на кошелёк (сумма — из записанного перевода).
fn deposit(hash: &str) -> BalanceChange {
    BalanceChange {
        venue: VenueId("bsc_dex".into()),
        asset: usdt(),
        delta: "58.69".parse().unwrap(),
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: Some(hash.to_string()),
        at: Utc::now(),
        source: "Transfer в блоке 129702236".into(),
    }
}

fn ticket(core: &Core) -> credit_watch::Ticket {
    let amount: Decimal = "58.69".parse().unwrap();
    credit_watch::register(
        core,
        "d1",
        &VenueId("bingx".into()),
        &VenueId("bsc_dex".into()),
        &usdt(),
        amount,
    )
}

#[tokio::test]
async fn sender_hash_matches_closes() {
    let (core, s) = start("match").await;
    configure(&s, true).await;
    let t = ticket(&core);
    assert!(t.set_tx(OUR).is_empty());
    credit_event::on_change(&core, deposit(OUR)).await;
    let hit = transfer_credit::by_event(Some(&t)).expect("закрыто");
    assert_eq!(hit.delta, "58.69".parse::<Decimal>().unwrap());
}

#[tokio::test]
async fn no_sender_hash_only_wakes() {
    let (core, s) = start("wake").await;
    configure(&s, true).await;
    let t = ticket(&core);
    credit_event::on_change(&core, deposit(OUR)).await;
    assert!(
        transfer_credit::by_event(Some(&t)).is_none(),
        "хеша отправителя нет — не закрыто"
    );
    // Пробуждение опроса — разрешение уже стоит.
    tokio::time::timeout(Duration::from_millis(10), t.wake.notified())
        .await
        .expect("опрос разбужен");
    assert!(t.set_tx(OUR).is_empty());
    assert!(
        transfer_credit::by_event(Some(&t)).is_some(),
        "хеш совпал — закрыто"
    );
}

#[tokio::test]
async fn foreign_hash_never_closes() {
    let (core, s) = start("foreign").await;
    configure(&s, true).await;
    let other = foreign();
    let t = ticket(&core);
    // Чужой перевод той же суммы до хеша отправителя: прикреплён, но не закрывает; снимается при set_tx.
    credit_event::on_change(&core, deposit(&other)).await;
    assert!(transfer_credit::by_event(Some(&t)).is_none());
    let dropped = t.set_tx(OUR);
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].tx_hash.as_deref(), Some(other.as_str()));
    assert!(transfer_credit::by_event(Some(&t)).is_none());
    // Чужой хеш после хеша отправителя — не прикрепляется.
    credit_event::on_change(&core, deposit(&other)).await;
    assert!(!t.has_event());
    // Настоящее событие закрывает.
    credit_event::on_change(&core, deposit(OUR)).await;
    assert!(transfer_credit::by_event(Some(&t)).is_some());
}

#[tokio::test]
async fn closing_off_attaches_without_closing() {
    let (core, s) = start("off").await;
    configure(&s, false).await;
    let t = ticket(&core);
    t.set_tx(OUR);
    credit_event::on_change(&core, deposit(OUR)).await;
    assert!(t.has_event(), "событие сопоставлено");
    assert!(
        transfer_credit::by_event(Some(&t)).is_none(),
        "закрытие выключено"
    );
}
