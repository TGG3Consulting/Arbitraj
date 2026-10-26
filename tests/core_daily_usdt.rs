//! Файл: tests/core_daily_usdt.rs
//! Что делает: решения 53 и 54 на настоящей SQLite во временной папке (ядро без площадок в реестре):
//! остановленная сделка и «Беру на себя» — предварительный убыток в итог дня и стоп по лимиту; закрытие —
//! факт вместо предварительного; итог дня после перезапуска восстанавливается; разбор в другой день —
//! коррекция в день разбора; алерт «мало USDT» — один раз на пересечение порога, состояние после перезапуска.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets. Моков нет.

use std::path::PathBuf;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::risk::Mode;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
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
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings)
}

/// Сделка на тикере из записи BingX: вложено 100, затраты 1, выручки нет (монеты у бота).
fn deal(id: &str) -> Deal {
    let d = Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut x = Deal::new(id.into(), d, json!(null), Decimal::ONE);
    x.invested = Decimal::from(100);
    x.spent = Decimal::ONE;
    x
}

#[tokio::test]
async fn halted_provisional_then_fact_and_restart() {
    let d = dir("daily");
    {
        let (core, settings) = start(&d).await;
        settings
            .set(
                "risk.max_daily_loss_usd",
                Scope::Global,
                Some(json!("50")),
                "тест",
            )
            .await
            .unwrap();
        core.set_mode(Mode::Running).await;
        let mut x = deal("d1");
        arb::core::ledger::finish(&core, &mut x, Step::Halted, "тест").await;
        let day = core.control().day;
        assert_eq!(day.result_usd, Decimal::from(-101));
        assert_eq!(day.provisional_usd, Decimal::from(-101));
        assert_eq!(x.provisional_usd, Some(Decimal::from(-101)));
        // Предварительный убыток 101 ≥ лимита 50 — стоп.
        assert_eq!(core.control().mode, Mode::Stopped);
    }
    // После перезапуска итог дня восстановлен; закрытие сделки заменяет предварительное фактом.
    let (core, _) = start(&d).await;
    assert_eq!(core.control().day.result_usd, Decimal::from(-101));
    let mut x = core.deals().into_iter().find(|x| x.id == "d1").unwrap();
    x.proceeds = Decimal::from(95);
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "разбор").await;
    let day = core.control().day;
    assert_eq!(day.result_usd, Decimal::from(-6));
    assert_eq!(day.final_usd, Decimal::from(-6));
    assert!(day.provisional.is_empty());
}

#[tokio::test]
async fn earlier_day_correction_goes_to_settle_day() {
    let d = dir("daily-yday");
    {
        // Вчера: предварительный −101 по сделке d2 (строка таблицы daily_result за вчера).
        let db = Db::open(&d.join("t.sqlite")).unwrap();
        let y = (chrono::Utc::now().date_naive() - chrono::Duration::days(1)).to_string();
        let body = json!({"final_usd": "-101", "provisional": {"d2": ["-101", y]}}).to_string();
        db.call(move |c| {
            c.execute(
                "INSERT INTO daily_result (day, body, updated_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![y, body, "t"],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
    }
    let (core, _) = start(&d).await;
    let day = core.control().day;
    // Сегодня — новый день: вчерашний предварительный в итог не входит, но ждёт разбора.
    assert_eq!(day.result_usd, Decimal::ZERO);
    assert_eq!(day.provisional.len(), 1);
    let mut x = deal("d2");
    x.proceeds = Decimal::from(95);
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "разбор на следующий день").await;
    // Коррекция в день разбора: факт −6 минус учтённые вчера −101 = +95.
    assert_eq!(core.control().day.result_usd, Decimal::from(95));
}

#[tokio::test]
async fn low_usdt_once_per_crossing_and_survives_restart() {
    let d = dir("lowusdt");
    let bx = VenueId("bingx".into());
    {
        let (core, settings) = start(&d).await;
        // Не задано — алерта нет, состояние не меняется.
        arb::core::low_usdt::check(&core, &bx, Decimal::from(10), "USDT").await;
        assert!(core.inner.usdt_below.lock().unwrap().get(&bx).is_none());
        settings
            .set(
                "alert.min_usdt",
                Scope::Venue("bingx".into()),
                Some(json!("50")),
                "тест",
            )
            .await
            .unwrap();
        arb::core::low_usdt::check(&core, &bx, Decimal::from(50), "USDT").await;
        assert_eq!(core.inner.usdt_below.lock().unwrap().get(&bx), Some(&true));
    }
    let (core, _) = start(&d).await;
    // Состояние «ниже порога» восстановлено — повторного алерта после перезапуска нет.
    assert_eq!(core.inner.usdt_below.lock().unwrap().get(&bx), Some(&true));
    arb::core::low_usdt::check(&core, &bx, Decimal::from(51), "USDT").await;
    assert_eq!(core.inner.usdt_below.lock().unwrap().get(&bx), Some(&false));
}
