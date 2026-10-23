//! Файл: tests/core_ledger_control.rs
//! Что делает: проверяет ядро целиком на настоящей SQLite во временной папке (без площадок в реестре):
//! после запуска система в режиме «Стоп»; торговля не включается, пока не заданы money.* и risk.*;
//! кнопка Telegram из чужого чата отклоняется; незакрытая сделка после перезапуска помечается
//! «прервана перезапуском»; алерта нет — команда отклоняется.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets. Моков нет.

use std::path::PathBuf;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::risk::Mode;
use arb::core::types::{Asset, CommandSource, Direction, ManualAction, ManualCommand, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-core-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Запускает ядро на базе в папке. Возвращает ядро и хранилище настроек.
async fn start(d: &std::path::Path) -> (Core, Store) {
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
    })
    .await;
    (core, settings)
}

/// Направление на тикере из записи BingX (tests/data/bingx/symbols_spot_2026-10-23.json).
fn cake() -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    }
}

#[tokio::test]
async fn stopped_until_start_and_settings() {
    let d = dir("control");
    let (core, _) = start(&d).await;
    assert_eq!(core.control().mode, Mode::Stopped);
    core.set_mode(Mode::Running).await;
    let err = arb::core::risk::can_open(&core, &cake()).unwrap_err();
    assert!(err.contains("money."), "{err}");
}

#[tokio::test]
async fn telegram_only_from_configured_chat() {
    let d = dir("tg");
    let (core, settings) = start(&d).await;
    settings
        .set(
            "notify.telegram_chat",
            Scope::Global,
            Some(json!("100")),
            "тест",
        )
        .await
        .unwrap();
    let cmd = |chat: &str| ManualCommand {
        action: ManualAction::SellNow {
            alert_id: "нет".into(),
        },
        source: CommandSource::Telegram { chat: chat.into() },
        at: Utc::now(),
    };
    let e = core.command(cmd("200")).await.unwrap_err();
    assert!(e.contains("не настроенный чат"), "{e}");
    let e = core.command(cmd("100")).await.unwrap_err();
    assert!(e.contains("алерта"), "{e}");
}

#[tokio::test]
async fn open_deal_halted_after_restart() {
    let d = dir("restart");
    {
        let (core, _) = start(&d).await;
        let mut deal = Deal::new("t1CAKE".into(), cake(), json!(null), Decimal::ONE);
        deal.step = Step::Transfer;
        arb::core::ledger::save(&core, &deal).await;
    }
    let (core, _) = start(&d).await;
    let deals = core.deals();
    assert_eq!(deals.len(), 1);
    assert_eq!(deals[0].step, Step::Halted);
}
