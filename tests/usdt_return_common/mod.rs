//! Файл: tests/usdt_return_common/mod.rs
//! Что делает: общее для тестов решения 163 (tests/core_usdt_return_retry.rs, tests/core_usdt_return_manual.rs):
//! ядро на настоящей SQLite во временной папке без площадок в реестре (как tests/core_usdt_return_close.rs),
//! usdt_return.mode = per_trade, venue.quote_asset = USDT у mexc и bingx, по желанию — повторы usdt.return_retry_s и
//! usdt.return_retries_max; сделка как живая 20261104225504576DOGE (bingx → mexc, выручка 83.10 USDT на mexc).
//! Кто вызывает: tests/core_usdt_return_*.rs (start_with — tests/core_usdt_return_tolerance.rs).
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets. Моков нет.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arb::core::Core;
use arb::core::deal::Deal;
use arb::core::deps::Deps;
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal};
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Папка теста (новая).
pub fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-u163-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе в папке. retry — (usdt.return_retry_s, usdt.return_retries_max); None — не задавать.
/// Возвращает ядро и журнал.
pub async fn start(d: &Path, retry: Option<(u64, u64)>) -> (Core, Journal) {
    start_with(d, retry, Vec::new()).await
}

/// Как start, плюс свои настройки extra — (имя, область, значение) — до запуска ядра.
pub async fn start_with(
    d: &Path,
    retry: Option<(u64, u64)>,
    extra: Vec<(&str, Scope, serde_json::Value)>,
) -> (Core, Journal) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let mut set = vec![("usdt_return.mode", json!("per_trade"))];
    if let Some((s, n)) = retry {
        set.push(("usdt.return_retry_s", json!(s)));
        set.push(("usdt.return_retries_max", json!(n)));
    }
    for (k, v) in set {
        settings
            .set(k, Scope::Global, Some(v), "тест")
            .await
            .unwrap();
    }
    for (k, sc, v) in extra {
        settings.set(k, sc, Some(v), "тест").await.unwrap();
    }
    // Котируемая монета — настройка площадки (у mexc и bingx — USDT).
    for v in ["mexc", "bingx"] {
        let sc = Scope::Venue(v.into());
        settings
            .set("venue.quote_asset", sc, Some(json!("USDT")), "тест")
            .await
            .unwrap();
    }
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, journal)
}

/// Сделка как живая DOGE: куплено на bingx, продано на mexc, вложено 80, выручка 83.10 USDT (на mexc).
pub fn doge(id: &str) -> Deal {
    let d = Direction {
        asset: Asset {
            ticker: "DOGE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("mexc".into()),
    };
    let mut x = Deal::new(id.into(), d, json!(null), Decimal::ONE);
    x.at = VenueId("mexc".into());
    x.invested = Decimal::from(80);
    x.proceeds = Decimal::new(8310, 2);
    x
}

/// Записи журнала с этим видом и подстрокой. Ошибок нет.
pub async fn entries(j: &Journal, kind: &str, part: &str) -> Vec<Entry> {
    let all = j.list(1000, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == kind && e.message.contains(part))
        .collect()
}

/// Ждёт, пока состояние возврата сделки не удовлетворит ok (не дольше max). Возвращает последнее состояние.
pub async fn wait_state(
    core: &Core,
    id: &str,
    max: Duration,
    ok: impl Fn(&UsdtReturn) -> bool,
) -> Option<UsdtReturn> {
    let t = Instant::now();
    loop {
        let s = core.deals().into_iter().find(|d| d.id == id)?.usdt_return;
        if s.as_ref().is_some_and(&ok) || t.elapsed() >= max {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
