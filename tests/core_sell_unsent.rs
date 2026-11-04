//! Файл: tests/core_sell_unsent.rs
//! Что делает: решение 154 на настоящей SQLite во временной папке (ядро без площадок в реестре, моков нет).
//! Живой случай 04.11.2026, сделка 20261104120743802ASTER (bingx → bsc_dex): 143.607219 ASTER зачислено на кошелёк,
//! продажа «свой сборщик: прямой» — «Ни один канал не принял транзакцию: … код -32000 …» (текст — из хронологии
//! сделки, тексты каналов там обрезаны), повтора не было, 4 минуты до кнопки человека, строка в хронологии — дважды.
//! Проверяется: (1) отказ всех каналов без факта транзакции → повтор с паузой stuck.reject_retry_pause_ms; с фактом
//! транзакции, кнопкой человека или без настройки — нет; (2) повтор уходит в режим застрявшей монеты и продаёт заново
//! с новой котировкой (здесь площадки нет — новая котировка честно «невозможна», сделка остановлена, но путь пройден);
//! (3) исчерпан stuck.unsent_retries_max → журнал Error core.sell_unsent_exhausted и алерт человеку, строка об отказе в
//! хронологии — одна; (4) адаптер: где транзакция по ответу eth_getTransactionByHash (запись узла BSC 2026-10-25).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets, arb::venues::bsc_dex::send.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::sell_unsent::{self, Retry};
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
use arb::venues::bsc_dex::send::absent;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Текст отказа из хронологии сделки ASTER (как записан; тексты каналов в хронологии обрезаны).
const ASTER: &str = "[свой сборщик: прямой] Ни один канал не принял транзакцию: <канал>: … код -32000: …; <канал>: … код -32000: …; <канал>: … код -32000: …";

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-unsent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе в папке; возвращает ядро, настройки и журнал.
async fn start(d: &Path) -> (Core, Store, Journal) {
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
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings, journal)
}

/// Задаёт общую настройку.
async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Сделка ASTER: монеты зачислены на кошелёк DEX, идёт продажа.
fn aster() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "ASTER".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut x = Deal::new(
        "20261104120743802ASTER".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.at = VenueId("bsc_dex".into());
    x.qty = Decimal::from_str("143.607219").unwrap();
    x.step = Step::Selling;
    x
}

/// Ошибка продажи: отказ всех каналов; sent — был ли факт транзакции в сети.
fn rejected(sent: bool) -> OpError {
    OpError {
        code: Some(ErrorCode::ChannelRejected),
        message: ASTER.into(),
        sent,
    }
}

/// Сделка по id из учёта ядра.
fn deal_of(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|d| d.id == id).unwrap()
}

#[tokio::test]
async fn decide_retries_only_unsent_channel_reject() {
    let d = dir("decide");
    let (core, s, _) = start(&d).await;
    let deal = aster();
    let e = rejected(false);
    // Как в живом случае до решения 154: повторы не настроены — алерт с причиной, почему бот сам не повторил.
    assert!(
        matches!(sell_unsent::decide(&core, &deal, &e, "выгодно"), Retry::Off(w) if w.contains("stuck.reject_retry_pause_ms"))
    );
    set(&s, "stuck.reject_retry_pause_ms", json!(1500)).await;
    assert!(
        matches!(sell_unsent::decide(&core, &deal, &e, "выгодно"), Retry::Off(w) if w.contains("stuck.unsent_retries_max"))
    );
    set(&s, "stuck.unsent_retries_max", json!(2)).await;
    assert_eq!(
        sell_unsent::decide(&core, &deal, &e, "выгодно"),
        Retry::Go { pause_ms: 1500 }
    );
    // Транзакция ушла в сеть — стартовое правило, не повторяем; кнопку человека — тоже (решение 110).
    assert_eq!(
        sell_unsent::decide(&core, &deal, &rejected(true), "выгодно"),
        Retry::No
    );
    assert_eq!(
        sell_unsent::decide(&core, &deal, &e, arb::core::sell::BUTTON),
        Retry::No
    );
    // Решение 110 не изменилось: отказ до отправки — без паузы.
    let quote = OpError {
        code: Some(ErrorCode::QuoteMismatch),
        message: "цена сборки хуже".into(),
        sent: false,
    };
    assert_eq!(
        sell_unsent::decide(&core, &deal, &quote, "выгодно"),
        Retry::Go { pause_ms: 0 }
    );
    let mut tired = aster();
    tired.unsent_retries = 2;
    assert_eq!(
        sell_unsent::decide(&core, &tired, &e, "выгодно"),
        Retry::Exhausted { max: 2 }
    );
}

#[tokio::test]
async fn retry_goes_to_new_sale_then_exhausted_alerts_once() {
    let d = dir("flow");
    let (core, s, journal) = start(&d).await;
    set(&s, "stuck.unsent_retries_max", json!(1)).await;
    set(&s, "stuck.reject_retry_pause_ms", json!(1)).await;
    set(&s, "stuck.mode", json!("wait")).await;
    set(&s, "stuck.wait_ms", json!(1)).await;
    let e = rejected(false);

    // Повтор: счётчик, запись, пауза, режим застрявшей монеты → продажа по рынку с новой котировкой.
    sell_unsent::retry(&core, aster(), &e, 1).await;
    let x = deal_of(&core, "20261104120743802ASTER");
    assert_eq!(x.unsent_retries, 1);
    let lines: Vec<&str> = x.timeline.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        lines
            .iter()
            .any(|t| t.contains("повтор через 1 мс через режим застрявшей монеты (1/1)"))
    );
    assert!(
        lines
            .iter()
            .any(|t| t.contains("Застрявшая монета: режим wait"))
    );
    // Площадки в реестре нет — новая котировка невозможна: путь новой продажи пройден, сделка остановлена честно.
    assert!(lines.iter().any(|t| t.contains("продажа по рынку")));

    // Второй отказ: предел исчерпан → журнал Error и алерт человеку; строка об отказе — одна.
    let mut y = aster();
    y.unsent_retries = 1;
    let r = sell_unsent::decide(&core, &y, &e, "выгодно");
    assert_eq!(r, Retry::Exhausted { max: 1 });
    sell_unsent::alert(&core, y, &e, &r).await;
    let y = deal_of(&core, "20261104120743802ASTER");
    assert_eq!(y.step, Step::WaitingHuman);
    let fails = y
        .timeline
        .iter()
        .filter(|(_, t)| t.contains("Ни один канал не принял"))
        .count();
    assert_eq!(
        fails, 1,
        "строка отказа в хронологии — одна: {:?}",
        y.timeline
    );
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("повторы исчерпаны") && t.contains("ждём решения человека"))
    );
    let log = journal.list(50, None).await.unwrap();
    assert!(
        log.iter()
            .any(|j| j.kind == "core.sell_unsent_exhausted" && matches!(j.level, Level::Error))
    );
}

#[test]
fn node_says_where_the_tx_is() {
    // Настоящий ответ узла BSC на eth_getTransactionByHash (вывод BNB с BingX, 2026-10-25): транзакция в блоке.
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string(
            "tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json",
        )
        .unwrap(),
    )
    .unwrap();
    let tx = rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "tx")
        .unwrap()["response"]["result"]
        .clone();
    assert_eq!(absent::located(&tx).as_deref(), Some("в блоке 0x7b7d27d"));
    // Та же транзакция до блока: blockNumber = null (execution-apis transaction.yaml) — в пуле узла, повтор опасен.
    let mut pending = tx.clone();
    pending["blockNumber"] = Value::Null;
    assert_eq!(absent::located(&pending).as_deref(), Some("в пуле узла"));
    // Узел не знает транзакцию: result = null (execution-apis transaction.yaml) — точно не ушла.
    assert_eq!(absent::located(&Value::Null), None);
}
