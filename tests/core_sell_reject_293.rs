//! Файл: tests/core_sell_reject_293.rs
//! Что делает: ревью решения 293 (защита от серии) на живом ядре (настоящая SQLite во временной папке, моков нет):
//! в реестре — настоящий адаптер BingX без сети (справочник пар — записанный ответ symbols SHOPINX-USDT 15.11.2026,
//! tests/data/bingx_minqty_2026-11-15). Продажа SHOPINX на BingX отклонена: текст отказа — настоящий ответ правил ордера
//! адаптера (order_rules::ready на записанном справочнике: «количество 11000 меньше minQty 57505» — так откажет и
//! локальная проверка при «да → нет», и, по смыслу, биржа). Проверяется:
//! 1) sell_unsent::decide — Retry::No (стартовое правило), alert — сделка ждёт человека (WaitingHuman);
//! 2) после alert направление этой сделки (DEX → BingX, SHOPINX) на паузе signal.route_cooldown_s: на ценах, на которых
//!    до отказа был «кандидат», фильтр сигнала даёт Cooldown (новая сделка начинается только из Candidate —
//!    core/evaluate.rs); обратное направление не на паузе;
//! 3) signal.route_cooldown_s не задана — паузы нет (как ledger::finish_paused), Warn в журнале core.ledger;
//! 4) отказ продажи на площадке не из реестра бирж (DEX) — паузы нет (rejected_on_cex).
//!
//! Цены сигнала — первый сопоставленный лот отчёта о чужих сделках SHOPINX 13.11 (покупка 100 USDT → 8362.877206745749506115
//! SHOPINX на DEX; продажа на BingX 8362.877206 → 103.63605112047 USDT, публичная лента исполнений), комиссии 0.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (engine, sell_unsent, signal, deal), arb::venues::bingx, arb::settings, arb::storage.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::sell_unsent::{self, Retry};
use arb::core::signal::{Params, Prices, State};
use arb::core::types::{Asset, Direction, Side, TradeRequest, VenueId};
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
        qty: d("11000"),
        worst_price: d("0.00870"),
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
        ticker: "SHOPINX".into(),
        bsc_contract: None,
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
    x.qty = d("11000");
    x.step = Step::Selling;
    x
}

/// Состояние фильтра сигнала направления на ценах первого лота (кандидат без паузы).
fn state(core: &Core, dir: &Direction) -> State {
    let p = Prices {
        buy: d("100") / d("8362.877206745749506115"),
        sell: d("103.63605112047") / d("8362.877206"),
        fees_pct: Decimal::ZERO,
        qty: d("8362.877206"),
    };
    let par = Ok(Params {
        hold_ms: 0,
        hold_updates: 1,
        anomaly_pct: d("100"),
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
    }
}

#[tokio::test]
async fn rejected_sale_pauses_direction() {
    let p = dir("pause");
    let (core, s, _, why) = start(&p).await;
    assert!(why.contains("minQty 57505"), "{why}");
    s.set(
        "signal.route_cooldown_s",
        Scope::Global,
        Some(json!(600)),
        "тест",
    )
    .await
    .unwrap();
    let dir = dex_to_bingx();
    assert_eq!(state(&core, &dir), State::Candidate);
    let x = deal("bingx");
    let e = rejected(&why);
    assert!(sell_unsent::rejected_on_cex(&core, &x, &e));
    let r = sell_unsent::decide(&core, &x, &e, "выгодно");
    assert_eq!(r, Retry::No);
    sell_unsent::alert(&core, x.clone(), &e, &r).await;
    let now = core.deals().into_iter().find(|v| v.id == x.id).unwrap();
    assert_eq!(now.step, Step::WaitingHuman);
    assert!(matches!(state(&core, &dir), State::Cooldown { .. }));
    // Обратное направление (BingX → DEX) не на паузе.
    let back = Direction {
        asset: asset(),
        buy_on: dir.sell_on.clone(),
        sell_on: dir.buy_on.clone(),
    };
    assert_eq!(state(&core, &back), State::Candidate);
}

#[tokio::test]
async fn no_setting_or_not_cex_no_pause() {
    let p = dir("nopause");
    let (core, _, journal, why) = start(&p).await;
    let dir = dex_to_bingx();
    // signal.route_cooldown_s не задана — как ledger::finish_paused: паузы нет, Warn в журнале core.ledger.
    let x = deal("bingx");
    let e = rejected(&why);
    sell_unsent::alert(&core, x, &e, &Retry::No).await;
    assert_eq!(state(&core, &dir), State::Candidate);
    let log = journal.list(500, None).await.unwrap();
    assert!(log.iter().any(|j| j.kind == "core.ledger"
        && j.level == Level::Warn
        && j.message.contains("signal.route_cooldown_s")));
    // Монеты на площадке не из реестра бирж (DEX) — не «отказ биржи».
    assert!(!sell_unsent::rejected_on_cex(&core, &deal("bsc_dex"), &e));
}
