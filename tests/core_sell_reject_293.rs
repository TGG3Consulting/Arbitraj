//! Файл: tests/core_sell_reject_293.rs
//! Что делает: ревью решения 293 (защита от серии) на живом ядре (настоящая SQLite во временной папке, моков нет):
//! в реестре — настоящий адаптер BingX без сети (справочник пар — записанный ответ symbols SHOPINX-USDT 15.11.2026,
//! tests/data/bingx_minqty_2026-11-15). Продажа SHOPINX на BingX отклонена: текст отказа — настоящий ответ правил ордера
//! адаптера (order_rules::ready на записанном справочнике: «количество 11000 меньше minQty 57505» — так откажет и
//! локальная проверка при «да → нет»). Путь: sell_unsent::decide = Retry::No → alert → сделка ждёт человека.
//! 1) stuck.reject_route_pause не задана — как на 52ca0c10: паузы нет и записей core.ledger нет (и при заданной
//!    signal.route_cooldown_s); «да» у системы и «нет» у площадки — паузы нет (площадка сильнее);
//! 2) «да» — пауза ровно одного направления сделки (DEX → BingX, SHOPINX) и одна запись Info core.ledger о ней: на ценах, где до отказа был «кандидат»,
//!    фильтр сигнала даёт Cooldown (новая сделка — только из Candidate, core/evaluate.rs); обратное — не на паузе;
//! 3) «да», а signal.route_cooldown_s не задана — паузы нет, Warn в журнале core.ledger (как ledger::finish_paused);
//! 4) rejected_on_cex: площадки нет в реестре — нет; площадка вида DEX — нет; BingX — да.
//!
//! Цены сигнала — лот 1 отчёта о чужих сделках SHOPINX 13.11 (tests/data/shopinx_trades_2026-11-15/lot1.txt: покупка
//! 100 USDT → 8362.877206745749506115 SHOPINX на DEX; «Публичные исполнения: 8362.877206 SHOPINX / 103.63605112047
//! USDT» на BingX), комиссии 0.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/bingx_293_common, arb::core (engine, sell_unsent, route_pause, signal, deal),
//! arb::venues::bingx, arb::settings, arb::storage.

mod bingx_293_common;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::route_pause;
use arb::core::sell_unsent::{self, Retry};
use arb::core::signal::{Params, Prices, State};
use arb::core::types::{Asset, Direction, Side, TradeRequest, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
use arb::storage::raw_log::RawLog;
use arb::venues::bingx::{Bingx, ctx::Ctx, order_rules, symbols_qty};
use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

const DATA: &str = "tests/data/bingx_minqty_2026-11-15";
/// Количество продажи — задано тестом: меньше записанного minQty 57505 SHOPINX-USDT (не данные биржи).
const QTY: &str = "11000";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("arb-293-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Ядро с адаптером BingX (без сети) в реестре; справочник пар — записанный ответ SHOPINX.
/// Возвращает: ядро, настройки, журнал, текст отказа правил ордера адаптера на 11 000 SHOPINX.
async fn start(p: &Path) -> (Core, Store, Journal, String) {
    let db = Db::open(&p.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let b = Scope::Venue("bingx".into());
    for (k, v) in [
        ("venue.quote_asset", "USDT"),
        ("venue.order_time_in_force", "IOC"),
    ] {
        settings
            .set(k, b.clone(), Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    let vault = arb::secrets::Vault::locked(p.join("secrets.enc"));
    let ctx = Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: RawLog::start(p.join("raw"), settings.clone()),
        taps: Default::default(),
    };
    let bx = Bingx::new(ctx.clone());
    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{DATA}/symbols_SHOPINX-USDT.json")).unwrap(),
    )
    .unwrap();
    let at = Utc.timestamp_millis_opt(1_794_742_830_000).unwrap();
    symbols_qty::take(&ctx, &bx.symbols(), &raw["data"], at).unwrap();
    let req = TradeRequest {
        venue: VenueId("bingx".into()),
        asset: asset(),
        side: Side::Sell,
        qty: d(QTY),
        // Записанный bid bookTicker SHOPINX-USDT 15.11 (tests/data/bingx_minqty_2026-11-15).
        worst_price: bingx_293_common::bid("SHOPINX-USDT"),
        deal_id: "t293".into(),
        route: None,
        exit_position: true,
        remeasure: None,
    };
    let why = order_rules::ready(&ctx, &bx.symbols(), &req)
        .unwrap_err()
        .message;
    let gw = Registry::new();
    gw.register(Arc::new(bx)).await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw,
        settings: settings.clone(),
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: p.to_path_buf(),
    })
    .await;
    (core, settings, journal, why)
}

fn asset() -> Asset {
    Asset {
        chain: None,
        ticker: "SHOPINX".into(),
        contract: None,
    }
}

/// Направление сделки: покупка на DEX, продажа на BingX (как у чужих сделок SHOPINX).
fn dex_to_bingx() -> Direction {
    Direction {
        asset: asset(),
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("bingx".into()),
    }
}

/// Сделка: монеты на площадке `at`, идёт продажа 11 000 SHOPINX.
fn deal(at: &str) -> Deal {
    let mut x = Deal::new(
        "20261115T293SHOPINX".into(),
        dex_to_bingx(),
        json!(null),
        Decimal::ZERO,
    );
    x.at = VenueId(at.into());
    x.qty = d(QTY);
    x.step = Step::Selling;
    x
}

/// Число из строки отчёта после метки (до пробела).
fn after(t: &str, mark: &str) -> Decimal {
    let i = t.find(mark).unwrap() + mark.len();
    d(t[i..].split_whitespace().next().unwrap())
}

/// Состояние фильтра сигнала направления на ценах лота 1 отчёта (tests/data/shopinx_trades_2026-11-15/lot1.txt):
/// покупка «KyberSwap 100.00 <монет>», продажа «Публичные исполнения: <монет> SHOPINX / <USDT> USDT»; комиссии 0.
fn state(core: &Core, dir: &Direction) -> State {
    let t = bingx_293_common::lot1();
    let (spent, bought) = (after(&t, "KyberSwap "), after(&t, "KyberSwap 100.00 "));
    let (sold, got) = (after(&t, "Публичные исполнения: "), after(&t, "SHOPINX / "));
    let p = Prices {
        buy: spent / bought,
        sell: got / sold,
        fees_pct: Decimal::ZERO,
        qty: sold,
        buy_at: Utc::now(),
        sell_at: Utc::now(),
    };
    let par = Ok(Params {
        hold_ms: 0,
        hold_updates: 1,
        anomaly_pct: d("100"),
        ..Default::default()
    });
    let mut s = core.inner.signals.lock().unwrap();
    s.update(dir, Ok(p), &par, Utc::now()).state
}

/// Отказ продажи с текстом правил ордера адаптера.
fn rejected(why: &str) -> OpError {
    OpError {
        code: Some(arb::gateway::errors::ErrorCode::OrderRejected),
        message: why.into(),
        sent: false,
        cancelled: false,
        price_moved: false,
    }
}

/// Задать настройку в области (система или площадка).
async fn put(s: &Store, name: &str, sc: Scope, v: Value) {
    s.set(name, sc, Some(v), "тест").await.unwrap();
}

/// Сколько записей журнала core.ledger.
async fn ledger_logs(j: &Journal) -> usize {
    let log = j.list(500, None).await.unwrap();
    log.iter().filter(|e| e.kind == "core.ledger").count()
}

/// Отказ продажи на BingX и алерт (путь sell.rs → decide → alert). Возвращает: сделку после алерта.
async fn reject_and_alert(core: &Core, why: &str) -> Deal {
    let x = deal("bingx");
    let e = rejected(why);
    assert!(route_pause::rejected_on_cex(core, &x, &e));
    let r = sell_unsent::decide(core, &x, &e, "выгодно");
    assert_eq!(r, Retry::No);
    sell_unsent::alert(core, x.clone(), &e, &r).await;
    core.deals().into_iter().find(|v| v.id == x.id).unwrap()
}

#[tokio::test]
async fn not_set_is_as_before() {
    let p = dir("notset");
    let (core, s, journal, why) = start(&p).await;
    let dir = dex_to_bingx();
    // stuck.reject_route_pause не задана, signal.route_cooldown_s не задана — паузы нет, записей core.ledger нет.
    let before = ledger_logs(&journal).await;
    assert_eq!(reject_and_alert(&core, &why).await.step, Step::WaitingHuman);
    assert_eq!(state(&core, &dir), State::Candidate);
    assert_eq!(ledger_logs(&journal).await, before);
    // signal.route_cooldown_s задана, а stuck.reject_route_pause нет — паузы тоже нет.
    put(&s, "signal.route_cooldown_s", Scope::Global, json!(600)).await;
    reject_and_alert(&core, &why).await;
    assert_eq!(state(&core, &dir), State::Candidate);
    // «Да» у системы, «нет» у площадки BingX — значение площадки сильнее: паузы нет.
    put(&s, route_pause::REJECT_PAUSE, Scope::Global, json!(true)).await;
    put(
        &s,
        route_pause::REJECT_PAUSE,
        Scope::Venue("bingx".into()),
        json!(false),
    )
    .await;
    reject_and_alert(&core, &why).await;
    assert_eq!(state(&core, &dir), State::Candidate);
}

#[tokio::test]
async fn yes_pauses_one_direction() {
    let p = dir("pause");
    let (core, s, journal, why) = start(&p).await;
    assert!(why.contains("minQty 57505"), "{why}");
    put(&s, "signal.route_cooldown_s", Scope::Global, json!(600)).await;
    put(
        &s,
        route_pause::REJECT_PAUSE,
        Scope::Venue("bingx".into()),
        json!(true),
    )
    .await;
    let dir = dex_to_bingx();
    assert_eq!(state(&core, &dir), State::Candidate);
    assert_eq!(reject_and_alert(&core, &why).await.step, Step::WaitingHuman);
    assert!(matches!(state(&core, &dir), State::Cooldown { .. }));
    // Пауза поставлена — одна запись Info в журнале core.ledger: монета, направление, секунды.
    let log = journal.list(500, None).await.unwrap();
    let info: Vec<_> = log
        .iter()
        .filter(|j| {
            j.kind == "core.ledger"
                && j.level == Level::Info
                && j.message.contains("пауза направления")
        })
        .collect();
    assert_eq!(info.len(), 1);
    assert!(
        info[0].message.contains("SHOPINX bsc_dex → bingx на 600 с"),
        "{}",
        info[0].message
    );
    // Обратное направление (BingX → DEX) не на паузе.
    let back = Direction {
        asset: asset(),
        buy_on: dir.sell_on.clone(),
        sell_on: dir.buy_on.clone(),
    };
    assert_eq!(state(&core, &back), State::Candidate);
}

#[tokio::test]
async fn yes_without_cooldown_warns() {
    let p = dir("warn");
    let (core, s, journal, why) = start(&p).await;
    put(&s, route_pause::REJECT_PAUSE, Scope::Global, json!(true)).await;
    reject_and_alert(&core, &why).await;
    // signal.route_cooldown_s не задана — как ledger::finish_paused: паузы нет, Warn в журнале core.ledger.
    assert_eq!(state(&core, &dex_to_bingx()), State::Candidate);
    let log = journal.list(500, None).await.unwrap();
    assert!(log.iter().any(|j| j.kind == "core.ledger"
        && j.level == Level::Warn
        && j.message.contains("signal.route_cooldown_s")));
}

#[tokio::test]
async fn not_cex_is_not_exchange_reject() {
    let p = dir("dex");
    let (core, _, _, why) = start(&p).await;
    let e = rejected(&why);
    // Площадки нет в реестре — вид неизвестен.
    assert!(!route_pause::rejected_on_cex(&core, &deal("bsc_dex"), &e));
    // Площадка вида DEX в реестре видов ядра — не «отказ биржи».
    core.inner
        .kinds
        .write()
        .unwrap()
        .push((VenueId("bsc_dex".into()), VenueKind::Dex));
    assert!(!route_pause::rejected_on_cex(&core, &deal("bsc_dex"), &e));
    // BingX (Cex) — «отказ биржи».
    assert!(route_pause::rejected_on_cex(&core, &deal("bingx"), &e));
}
