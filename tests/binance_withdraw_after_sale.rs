//! Файл: tests/binance_withdraw_after_sale.rs
//! Что проверяет: решение 253, дополнение t89 (Тигран 11.11) — защита вывода сразу после своей сделки на Binance у
//! возврата USDT после продажи. Живой случай 11.11, сделка 20261111122340181OPEN (DEX → Binance): продажа 415.4 OPEN
//! исполнилась 12:24:21.980, возврат 39.714 USDT binance → bsc_dex начат 12:24:21.981, отказ 12:24:22.582
//! «-4026 [031033] User has insufficient balance» (текст отказа — из сделки):
//! - окно venue.withdraw_unready_window_ms — от своей ПРОДАЖИ (отметка котируемой монеты): −4026 по USDT в окне —
//!   WithdrawLocked, повтор по transfer.locked_* площадки; строки повторов — в хронологии закрытой сделки (ID вывода
//!   возврата = сделка + «u»);
//! - ожидание доступности USDT: событие остатков, полученное ДО своего ордера (на споте уже лежали USDT), не считается —
//!   ждём событие после продажи;
//! - пауза venue.withdraw_min_delay_ms — от своей сделки площадки по активу (контракт since_own_trade у Binance), у
//!   биржи без контракта — как раньше (нет покупки сделки — без паузы);
//! - монета после покупки — как было (окно по монете, сделка по ID вывода — сама сделка).
//!
//! Настоящее ядро на SQLite во временной папке и настоящий адаптер Binance (без сети). Моков нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::binance, arb::core (withdraw_after, withdraw_wait, transfer_locked).

mod usdt_return_common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::transfer_locked::{again, is_locked};
use arb::core::types::{Asset, VenueId};
use arb::core::withdraw_after::{Since, deal_key, from_trade, later};
use arb::gateway::contract::Venue;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::value::Scope;
use arb::venues::binance::errors::{Op, from_code};
use arb::venues::binance::{transfers, withdraw_ready};
use usdt_return_common::{dir, doge, start};

/// ID сделки живого случая.
const DEAL: &str = "20261111122340181OPEN";
/// Текст отказа Binance из сделки 20261111122340181OPEN.
const REFUSED: &str = "[031033] User has insufficient balance";

fn venue() -> VenueId {
    VenueId("binance".into())
}

fn asset(t: &str) -> Asset {
    Asset {
        ticker: t.into(),
        bsc_contract: None,
    }
}

/// Окружение адаптера Binance на настройках ядра. Ошибок нет.
fn bn(core: &arb::core::Core, d: &std::path::Path) -> arb::venues::binance::ctx::Ctx {
    let s = core.inner.deps.settings.clone();
    arb::venues::binance::ctx::Ctx {
        settings: s.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal: core.inner.deps.journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.join("raw"), s),
        taps: Default::default(),
    }
}

async fn set(core: &arb::core::Core, name: &str, v: Value) {
    core.inner
        .deps
        .settings
        .set(name, Scope::Venue("binance".into()), Some(v), "тест")
        .await
        .unwrap();
}

/// Отказ вывода −4026 с текстом из сделки (через настоящий разбор адаптера). Ошибок нет.
fn refused() -> GatewayError {
    let raw = json!({"code": -4026, "msg": REFUSED}).to_string();
    from_code(-4026, REFUSED, Op::Withdraw, &raw)
}

#[tokio::test]
async fn usdt_after_sale_window_and_retry() {
    let d = dir("bn89window");
    let (core, _) = start(&d, None).await;
    let ctx = bn(&core, &d);
    let e = refused();
    assert_eq!(e.code, ErrorCode::WithdrawRejected, "без окна — как раньше");
    // Своя продажа OPEN: адаптер отмечает монету сделки и котируемую монету (mod.rs trade) — здесь USDT89.
    let sent = tokio::time::Instant::now();
    withdraw_ready::traded("OPEN89", sent);
    withdraw_ready::traded("USDT89", sent);
    assert_eq!(
        transfers::not_ready(&ctx, "USDT89", e.clone()).code,
        ErrorCode::WithdrawRejected,
        "окно не задано — как раньше"
    );
    set(&core, "venue.withdraw_unready_window_ms", json!(16_000)).await;
    let x = transfers::not_ready(&ctx, "USDT89", e.clone());
    assert_eq!(x.code, ErrorCode::WithdrawLocked, "{}", x.message);
    assert!(x.message.contains("своя сделка по USDT89"), "{}", x.message);
    assert!(
        x.message.contains("ещё не доступна для вывода"),
        "{}",
        x.message
    );
    assert!(is_locked(&x), "вывод не создан — повтор безопасен");
    // Своей сделки по активу не было — как раньше.
    assert_eq!(
        transfers::not_ready(&ctx, "USDC89", e).code,
        ErrorCode::WithdrawRejected
    );
    // Повтор ядра по transfer.locked_* площадки; строки — в закрытую сделку (ID вывода возврата = сделка + «u»).
    set(&core, "transfer.locked_retry_ms", json!(20)).await;
    set(&core, "transfer.locked_retries_max", json!(2)).await;
    arb::core::ledger::save(&core, &doge(DEAL)).await;
    let mut done = 0;
    while again(&core, &format!("{DEAL}u"), &venue(), &x, &mut done).await {}
    assert_eq!(done, 2);
    let deals = arb::core::engine::lk(&core.inner.deals);
    let t = &deals.get(DEAL).unwrap().timeline;
    assert!(
        t.iter().any(|(_, s)| s.contains("Вывод: попытка 1 из 2")),
        "{t:?}"
    );
}

#[tokio::test]
async fn usdt_wait_ignores_balance_before_sale() {
    let d = dir("bn89wait");
    let (core, _) = start(&d, None).await;
    let ctx = bn(&core, &d);
    set(&core, "venue.withdraw_wallet", json!("spot")).await;
    let amount = Decimal::new(39_714, 3);
    // До продажи на споте уже лежали свои USDT — событие старое.
    withdraw_ready::on_position(&[("USDT89W".into(), Decimal::from(1000))], 1);
    withdraw_ready::traded("USDT89W", tokio::time::Instant::now());
    let t = Instant::now();
    let w = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            withdraw_ready::wait(&ctx, &asset("USDT89W"), amount, Duration::from_secs(5)).await
        }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!w.is_finished(), "старое событие (до продажи) не считается");
    // Событие после продажи — выручка зачислена.
    let after = Decimal::new(1_039_714, 3);
    withdraw_ready::on_position(&[("USDT89W".into(), after)], 2);
    let r = w.await.unwrap().unwrap();
    assert_eq!(r.ready, Some(true));
    assert_eq!(r.free, Some(after));
    assert!(t.elapsed() < Duration::from_secs(4), "вышли по событию");
}

#[tokio::test]
async fn pause_from_own_sale_only_where_reported() {
    let d = dir("bn89pause");
    let (core, _) = start(&d, None).await;
    let gw: Arc<dyn Venue> = Arc::new(arb::venues::binance::Binance::new(bn(&core, &d)));
    arb::core::ledger::save(&core, &doge(DEAL)).await;
    let now = chrono::Utc::now();
    // Своей сделки по USDT89P не было, покупки сделки на binance нет — паузы нет (как раньше).
    assert_eq!(
        from_trade(
            &core,
            &format!("{DEAL}u"),
            &venue(),
            &gw,
            &asset("USDT89P"),
            now
        ),
        None
    );
    // Продажа на Binance — пауза от неё (как в живом случае: возврат через 1 мс после продажи).
    withdraw_ready::traded("USDT89P", tokio::time::Instant::now());
    let now = chrono::Utc::now();
    let (at, since) = from_trade(
        &core,
        &format!("{DEAL}u"),
        &venue(),
        &gw,
        &asset("USDT89P"),
        now,
    )
    .unwrap();
    assert_eq!(since, Since::OwnTrade);
    let left = arb::core::withdraw_wait::left_ms(at, now, 1000).unwrap();
    assert!(left > Duration::from_millis(900), "{left:?}");
    // Биржа без контракта since_own_trade (MEXC) — без изменений: нет покупки сделки — нет паузы.
    let mx = bn(&core, &d);
    let mexc: Arc<dyn Venue> =
        Arc::new(arb::venues::mexc::Mexc::new(arb::venues::mexc::ctx::Ctx {
            settings: mx.settings,
            vault: mx.vault,
            http: mx.http,
            journal: mx.journal,
            raw: mx.raw,
            taps: mx.taps,
        }));
    let m = VenueId("mexc".into());
    assert_eq!(mexc.since_own_trade(&asset("USDT89P")), None);
    assert_eq!(
        from_trade(
            &core,
            &format!("{DEAL}u"),
            &m,
            &mexc,
            &asset("USDT89P"),
            now
        ),
        None
    );
}

#[test]
fn deal_by_withdraw_id_and_later() {
    let has = |k: &str| k == DEAL;
    assert_eq!(
        deal_key(has, DEAL).as_deref(),
        Some(DEAL),
        "перегон монеты — сама сделка"
    );
    for tag in ["u", "u1", "u12", "m123456"] {
        assert_eq!(
            deal_key(has, &format!("{DEAL}{tag}")).as_deref(),
            Some(DEAL),
            "{tag}"
        );
    }
    assert_eq!(
        deal_key(has, &format!("{DEAL}m")),
        None,
        "у «Перегнать» метка с цифрами"
    );
    assert_eq!(deal_key(has, &format!("{DEAL}x1")), None);
    assert_eq!(
        deal_key(has, "u20261111122340181"),
        None,
        "«отдельно» — свой ID"
    );
    let t = chrono::Utc::now();
    let a = Some((t, Since::DealBuy));
    let b = Some((t + chrono::Duration::milliseconds(5), Since::OwnTrade));
    assert_eq!(later(a, b), b);
    assert_eq!(later(b, a), b);
    assert_eq!(later(a, None), a);
    assert_eq!(later(None, None), None);
}
