//! Файл: tests/core_manual_sell_235.rs
//! Что делает: решение 235 на настоящей SQLite во временной папке (ядро без площадок в реестре, моков нет).
//! Требование Тиграна 08.11.2026: «Нажимаю "Продать" — не продаёт, потому что процент минуса больше настройки. Процент
//! минуса меня вообще не волнует, когда я руками нажимаю»; «Продать и перегнать» — один проход оценки всех площадок,
//! продажа там, где больше после всех затрат, возврат USDT на площадку покупки.
//! Проверяется: (1) исчерпание повторов кнопки — алерт (Exhausted), не продажа здесь по порогу; у автоматики — как
//! раньше (Here); (2) повтор кнопки после отказа до отправки идёт снова кнопкой (manual_sell.rs), а не в режим
//! застрявшей монеты: при заданных stuck.mode = relay и stuck.relay_max_loss_pct = 0 в хронологии нет порога, итог кнопки
//! «Не продано» с точной причиной; (3) продажа кнопкой без котировки — сделка остановлена с причиной и итогом кнопки,
//! порога нет; (4) выбор «Продать и перегнать» — наибольшая выручка после затрат против «здесь»; (5) проход оценки без
//! справочника монеты — только «здесь», строка оценки в хронологии; (6) итог кнопки — возврат USDT словами.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::secrets.

use std::str::FromStr;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::manual_outcome::return_text;
use arb::core::manual_relay::{self, Pick};
use arb::core::op_error::OpError;
use arb::core::sell::BUTTON;
use arb::core::sell_unsent::{self, Retry};
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use arb::core::view::CoreEvent;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Отказ пробы до отправки после кнопки (текст — из хронологии сделки JCT 08.11, решение 229).
const PROBE: &str = "[свой сборщик: прямой] проба свопа откатилась: TooLittleReceived";

/// Ядро на базе во временной папке; возвращает ядро и настройки.
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-234-{name}-{}", std::process::id()));
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

/// Задаёт общую настройку.
async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Площадка по id.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Сделка: куплено на mexc, монеты на кошельке DEX, идёт продажа кнопкой.
fn deal(id: &str) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "JCT".into(),
            bsc_contract: None,
        },
        buy_on: v("mexc"),
        sell_on: v("bsc_dex"),
    };
    let mut x = Deal::new(id.into(), dir, json!(null), Decimal::ZERO);
    x.at = v("bsc_dex");
    x.qty = Decimal::from(1000);
    x.invested = Decimal::from(10);
    x.step = Step::Selling;
    x
}

/// Отказ пробы до отправки (SwapReverted без факта транзакции — retry_safe).
fn probe() -> OpError {
    OpError::from(&GatewayError::new(
        ErrorCode::SwapReverted,
        Some(v("bsc_dex")),
        PROBE,
    ))
}

/// Сделка из учёта ядра по id.
fn get(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|d| d.id == id).unwrap()
}

/// Итоги кнопки (manual_outcome.rs) из событий ядра: (ok, текст).
fn finals(rx: &mut broadcast::Receiver<CoreEvent>) -> Vec<(bool, String)> {
    let mut out = vec![];
    while let Ok(ev) = rx.try_recv() {
        if let CoreEvent::Command(v) = ev
            && v["final"].as_bool() == Some(true)
        {
            out.push((
                v["ok"].as_bool().unwrap_or(false),
                v["text"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    out
}

/// В хронологии нет порогов убытка (stuck.relay_max_loss_pct, stuck.max_loss_pct) и режима застрявшей монеты.
fn no_threshold(d: &Deal) {
    for (_, t) in &d.timeline {
        assert!(
            !t.contains("max_loss_pct") && !t.contains("Застрявшая монета"),
            "{t}"
        );
    }
}

#[tokio::test]
async fn button_exhausted_alerts_auto_sells_here() {
    let (core, s) = start("decide").await;
    set(&s, "stuck.unsent_retries_max", json!(1)).await;
    let mut x = deal("234A");
    assert_eq!(
        sell_unsent::decide(&core, &x, &probe(), BUTTON),
        Retry::Go { pause_ms: 0 }
    );
    x.unsent_retries = 1;
    assert_eq!(
        sell_unsent::decide(&core, &x, &probe(), BUTTON),
        Retry::Exhausted { max: 1 }
    );
    // Автоматика — как раньше (решение 229): продажа здесь по порогу.
    assert_eq!(
        sell_unsent::decide(&core, &x, &probe(), "выгодно"),
        Retry::Here { max: 1 }
    );
}

#[tokio::test]
async fn button_retry_stays_button_without_threshold() {
    let (core, s) = start("retry").await;
    set(&s, "stuck.unsent_retries_max", json!(2)).await;
    set(&s, "stuck.mode", json!("relay")).await;
    set(&s, "stuck.relay_max_loss_pct", json!(0)).await;
    set(&s, "stuck.max_loss_pct", json!(0)).await;
    let mut rx = core.subscribe();
    arb::core::manual_sell::retry(&core, deal("234B"), &probe(), 0).await;
    let y = get(&core, "234B");
    // Площадки нет в реестре — баланс не прочитан: алерт с точной причиной, сделка ждёт человека.
    assert_eq!(y.step, Step::WaitingHuman, "{:?}", y.timeline);
    assert_eq!(y.unsent_retries, 1);
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("повтор кнопкой по рынку (1/2)")),
        "{:?}",
        y.timeline
    );
    no_threshold(&y);
    let f = finals(&mut rx);
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(
        !f[0].0 && f[0].1.starts_with("Не продано: повтор продажи кнопкой"),
        "{f:?}"
    );
}

#[tokio::test]
async fn button_without_quote_halts_with_reason() {
    let (core, s) = start("noquote").await;
    set(&s, "stuck.mode", json!("relay")).await;
    set(&s, "stuck.relay_max_loss_pct", json!(0)).await;
    let mut rx = core.subscribe();
    arb::core::sell::market(&core, deal("234C"), BUTTON).await;
    let y = get(&core, "234C");
    assert_eq!(y.step, Step::Halted, "{:?}", y.timeline);
    no_threshold(&y);
    let f = finals(&mut rx);
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(!f[0].0 && f[0].1.contains("невозможна"), "{f:?}");
}

#[test]
fn pick_best_after_costs() {
    let d = |s: &str| Decimal::from_str(s).unwrap();
    let t = vec![
        (v("bingx"), d("9.5")),
        (v("mexc"), d("11.2")),
        (v("gate"), d("-1")),
    ];
    assert_eq!(
        manual_relay::pick(Some(d("10")), &t),
        Pick::There(v("mexc"), d("11.2"))
    );
    // Здесь не хуже лучшей цели — продать здесь.
    assert_eq!(manual_relay::pick(Some(d("11.2")), &t), Pick::Here);
    // Здесь котировки нет — лучшая цель.
    assert_eq!(
        manual_relay::pick(None, &t),
        Pick::There(v("mexc"), d("11.2"))
    );
    // Цели не окупаются (≤ 0) или их нет — здесь.
    assert_eq!(manual_relay::pick(None, &[(v("gate"), d("0"))]), Pick::Here);
    assert_eq!(manual_relay::pick(Some(d("1")), &[]), Pick::Here);
}

#[tokio::test]
async fn sell_return_evaluates_then_sells_by_button() {
    let (core, s) = start("relay").await;
    set(&s, "stuck.relay_max_loss_pct", json!(0)).await;
    let mut rx = core.subscribe();
    let mut x = deal("234D");
    x.return_usdt = true;
    let (to, skip) = arb::core::manual_relay_targets::targets(&core, &x);
    assert!(to.is_empty() && skip.iter().any(|t| t.contains("монеты нет в справочнике")));
    manual_relay::run(&core, x).await;
    let y = get(&core, "234D");
    assert!(
        y.timeline.iter().any(|(_, t)| t
            .contains("«Продать и перегнать»: оценка площадок — здесь (bsc_dex): нет котировки")
            && t.contains("выбор — продать здесь (bsc_dex)")),
        "{:?}",
        y.timeline
    );
    assert_eq!(y.step, Step::Halted);
    assert_eq!(y.relays, 0);
    no_threshold(&y);
    let f = finals(&mut rx);
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(!f[0].0, "{f:?}");
}

#[test]
fn outcome_names_usdt_return() {
    let mut x = deal("234E");
    x.return_usdt = true;
    x.usdt_return = Some(UsdtReturn::Pending {
        amount: Decimal::from(12),
        from: v("bsc_dex"),
        to: v("mexc"),
    });
    let t = return_text(&x).unwrap();
    assert!(t.starts_with("возврат") && t.contains("начат"), "{t}");
    // Продано на площадке покупки — перегон не нужен.
    let mut y = deal("234F");
    y.return_usdt = true;
    y.at = v("mexc");
    assert!(return_text(&y).unwrap().contains("не нужен"));
    // Без «Продать и перегнать» и без частей — ничего.
    y.return_usdt = false;
    assert!(return_text(&y).is_none());
}
