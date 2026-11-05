//! Файл: tests/core_delist.rs
//! Что делает: решение 175 (делистинги) на настоящем ядре с настоящей SQLite во временной папке (ядро без площадок в
//! реестре, моков нет): делистинги бирж кладутся в состояние ядра так же, как их кладёт фоновая задача core/delist.rs
//! (DelistState::set), и проверяется решение перед сделкой (core/delist_check.rs) и что отказ доходит до расчёта
//! кандидата (core/profit.rs decide — та же функция, что у предпроверки перед покупкой).
//! Живой случай 05.11.2026 (сделка 20261105020010981ION, kucoin → mexc): KuCoin снял ION/USDT с торгов — объявление
//! https://www.kucoin.com/announcement/ae-kucoin-will-delist-multiple-tokens-on-2026-11-05: прекращение торгов 05.11
//! 00:00 «по времени биржи», вывод до 07.12 08:00 UTC. Часовой пояс «времени биржи» в брифе не уточнён — в тесте торги
//! до 05.11 00:00 UTC, «сейчас» — 04.11 21:00 UTC (как в примере причины «осталось 3 ч < 48 ч»).
//! Проверяется: делистинг на стороне продажи блокирует (MEXC → KuCoin), на стороне покупки блокирует (KuCoin → MEXC),
//! обе стороны без делистинга — проходит, «неизвестно» — не блокирует, время прошло — отказ, время не сообщено (BingX
//! pre-delisted) — отказ, порог не задан — проверка выключена, на стороне покупки закрытие вывода раньше торгов — по нему.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

use std::path::PathBuf;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::{Asset, DelistItem, Delistings, Direction, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;
use tokio::sync::broadcast;

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-delist-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Запускает ядро на базе в папке; порог delist.block_hours = 48 (Some) или не задан (None).
async fn start(d: &std::path::Path, hours: Option<&str>) -> Core {
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
    if let Some(h) = hours {
        settings
            .set("delist.block_hours", Scope::Global, Some(json!(h)), "тест")
            .await
            .unwrap();
    }
    core
}

/// Момент UTC.
fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
}

/// «Сейчас» живого случая: за 3 часа до прекращения торгов.
fn now() -> DateTime<Utc> {
    at(2026, 11, 4, 21)
}

/// Направление ION: покупка на buy, продажа на sell.
fn ion(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "ION".into(),
            bsc_contract: None,
        },
        buy_on: VenueId(buy.into()),
        sell_on: VenueId(sell.into()),
    }
}

/// Делистинг ION на KuCoin по объявлению живого случая.
fn kucoin_ion() -> Delistings {
    Delistings::of(vec![DelistItem {
        ticker: "ION".into(),
        symbol: "ION-USDT".into(),
        trading_end: Some(at(2026, 11, 5, 0)),
        withdraw_end: Some(at(2026, 12, 7, 8)),
        source: "объявление KuCoin 05.11.2026".into(),
    }])
}

/// Кладёт делистинги площадки в ядро (как фоновая задача core/delist.rs).
fn put(core: &Core, venue: &str, got: Delistings) {
    core.inner
        .delist
        .set(&VenueId(venue.into()), got, Utc::now());
}

#[tokio::test]
async fn sell_side_delist_blocks() {
    let core = start(&dir("sell"), Some("48")).await;
    put(&core, "kucoin", kucoin_ion());
    put(&core, "mexc", Delistings::of(vec![]));
    let err = arb::core::delist_check::check(&core, &ion("mexc", "kucoin"), now())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "ION: делистинг на KuCoin 05.11 00:00 UTC, осталось 3 ч < 48 ч"
    );
}

#[tokio::test]
async fn buy_side_delist_blocks() {
    let core = start(&dir("buy"), Some("48")).await;
    put(&core, "kucoin", kucoin_ion());
    put(&core, "mexc", Delistings::of(vec![]));
    let err = arb::core::delist_check::check(&core, &ion("kucoin", "mexc"), now())
        .await
        .unwrap_err();
    assert!(
        err.starts_with("ION: делистинг на KuCoin 05.11 00:00 UTC"),
        "{err}"
    );
    // Тот же отказ доходит до расчёта кандидата (и предпроверки — она зовёт decide).
    let v =
        arb::core::profit::decide(&core, &ion("kucoin", "mexc"), &arb::core::dry::Mode::Live).await;
    match v {
        arb::core::profit::Verdict::Reject { reason, best } => {
            assert!(reason.contains("делистинг на KuCoin"), "{reason}");
            assert!(best.is_none());
        }
        other => panic!("ожидался отказ, получено {other:?}"),
    }
}

#[tokio::test]
async fn no_delist_passes_and_far_delist_passes() {
    let core = start(&dir("pass"), Some("48")).await;
    put(&core, "kucoin", Delistings::of(vec![]));
    put(&core, "mexc", Delistings::of(vec![]));
    let d = ion("mexc", "kucoin");
    assert_eq!(
        arb::core::delist_check::check(&core, &d, now()).await,
        Ok(())
    );
    // Делистинг через 72 ч при пороге 48 ч — сделка проходит.
    put(&core, "kucoin", kucoin_ion());
    let early = at(2026, 11, 2, 0);
    assert_eq!(
        arb::core::delist_check::check(&core, &d, early).await,
        Ok(())
    );
    // Делистинг другой монеты — не мешает.
    let mut other = ion("mexc", "kucoin");
    other.asset.ticker = "CAKE".into();
    assert_eq!(
        arb::core::delist_check::check(&core, &other, now()).await,
        Ok(())
    );
}

#[tokio::test]
async fn unknown_does_not_block() {
    let core = start(&dir("unknown"), Some("48")).await;
    put(&core, "kucoin", Delistings::unknown("нет в документации"));
    put(&core, "mexc", Delistings::unknown("нет в документации"));
    let d = ion("kucoin", "mexc");
    assert_eq!(
        arb::core::delist_check::check(&core, &d, now()).await,
        Ok(())
    );
    // Площадка, о которой ответа ещё нет (или DEX), — тоже не блокирует.
    let dex = ion("bsc_dex", "mexc");
    assert_eq!(
        arb::core::delist_check::check(&core, &dex, now()).await,
        Ok(())
    );
}

#[tokio::test]
async fn passed_and_untimed_block() {
    let core = start(&dir("passed"), Some("48")).await;
    put(&core, "kucoin", kucoin_ion());
    let late = at(2026, 11, 5, 2);
    let err = arb::core::delist_check::check(&core, &ion("mexc", "kucoin"), late)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "ION: делистинг на KuCoin 05.11 00:00 UTC — время уже прошло"
    );
    // BingX: статус pre-delisted без времени — отказ.
    put(
        &core,
        "bingx",
        arb::venues::bingx::delist::build(vec![("ION".into(), "ION-USDT".into())]),
    );
    let err = arb::core::delist_check::check(&core, &ion("bingx", "mexc"), now())
        .await
        .unwrap_err();
    assert!(
        err.contains("делистинг на BingX — время не сообщено"),
        "{err}"
    );
}

#[tokio::test]
async fn unset_threshold_turns_check_off() {
    let core = start(&dir("unset"), None).await;
    put(&core, "kucoin", kucoin_ion());
    let d = ion("mexc", "kucoin");
    assert_eq!(
        arb::core::delist_check::check(&core, &d, now()).await,
        Ok(())
    );
}

#[test]
fn withdraw_close_counts_on_buy_side_only() {
    let item = DelistItem {
        ticker: "ION".into(),
        symbol: "ION-USDT".into(),
        trading_end: Some(at(2026, 11, 10, 0)),
        withdraw_end: Some(at(2026, 11, 5, 0)),
        source: "тест".into(),
    };
    let h = rust_decimal::Decimal::from(48);
    // Сторона покупки (вывод нужен): закрытие вывода раньше торгов — по нему.
    let err = arb::core::delist_check::verdict("kucoin", &item, true, now(), h).unwrap_err();
    assert_eq!(
        err,
        "ION: закрытие вывода на KuCoin 05.11 00:00 UTC, осталось 3 ч < 48 ч"
    );
    // Сторона продажи — только торги (через 5 суток) — проходит.
    assert_eq!(
        arb::core::delist_check::verdict("kucoin", &item, false, now(), h),
        Ok(())
    );
}
