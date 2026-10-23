//! Файл: tests/bsc_dex_send.rs
//! Что делает: проверяет правила отправки DEX без сети: send.max_wait_blocks < send.bundle_max_blocks — отправка
//! не работает с понятной причиной; цена газа отмены выше прошлой на надбавку; обмен без минимума (худшая цена ≤ 0,
//! допуск 100%) не собирается; ожидаемое поступление токена и обмен с этим токеном на выходе не идут одновременно.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{send, exec, wallet, ctx}, arb::settings (настоящая SQLite во временной
//! папке). Моков нет: узел не нужен — проверяются правила до отправки.

use std::path::PathBuf;
use std::time::Duration;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::{Asset, Pool, PoolRef, PoolState, Side, TradeRequest, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::trade;
use arb::venues::bsc_dex::pools::families::FAMILIES;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::send::{cancel, tx::SendCfg};
use arb::venues::bsc_dex::wallet::inflows::Inflows;

const COIN: Address = address!("0x1000000000000000000000000000000000000001");
const USDT: Address = address!("0x2000000000000000000000000000000000000002");

/// Окружение адаптера на настоящей SQLite во временной папке.
async fn ctx(name: &str) -> Ctx {
    let d: PathBuf = std::env::temp_dir().join(format!("arb-dex-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Задаёт настройку площадки bsc_dex.
async fn set(c: &Ctx, name: &str, v: serde_json::Value) {
    c.settings
        .set(name, Scope::Venue("bsc_dex".into()), Some(v), "тест")
        .await
        .unwrap();
}

#[tokio::test]
async fn max_wait_must_cover_bundle_blocks() {
    let c = ctx("cfg").await;
    assert_eq!(
        SendCfg::read(&c).unwrap_err().code,
        ErrorCode::NotConfigured
    );
    set(&c, "send.max_wait_blocks", json!(3)).await;
    set(&c, "send.bundle_max_blocks", json!(5)).await;
    set(&c, "send.cancel_gas_bump_pct", json!("12.5")).await;
    set(&c, "feed.stale_after_ms", json!(3000)).await;
    let e = SendCfg::read(&c).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotConfigured);
    assert!(e.message.contains("send.max_wait_blocks"));
    set(&c, "send.max_wait_blocks", json!(5)).await;
    let cfg = SendCfg::read(&c).unwrap();
    // Отмена: 100 wei × 1,125 = 112,5 → вверх 113; каналов нет — нижней границы нет.
    assert_eq!(
        cancel::price(&c, U256::from(100u8), &cfg, &[]).unwrap(),
        U256::from(113u8)
    );
}

/// Пара V2 «монета — USDT».
fn entry() -> Entry {
    let family = FAMILIES.iter().find(|f| f.id == "pancakeswap_v2").unwrap();
    Entry {
        family,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: family.id.into(),
            kind: family.kind,
            reference: PoolRef::Contract(COIN),
            token0: COIN,
            token1: USDT,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::from(1u8),
                reserve1: U256::from(1u8),
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: COIN,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
    }
}

fn req(worst: Decimal) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "X".into(),
            bsc_contract: Some(COIN),
        },
        side: Side::Sell,
        qty: Decimal::ONE,
        worst_price: worst,
        deal_id: "т".into(),
        route: None,
    }
}

#[tokio::test]
async fn no_swap_without_minimum() {
    let c = ctx("min").await;
    let e = entry();
    // Худшая цена 0 — минимум не считается (и нет деления на 0).
    let err = trade::order(
        &c,
        &req(Decimal::ZERO),
        &e,
        Decimal::ONE,
        (Decimal::ONE, Decimal::ONE),
        (18, 18),
    );
    assert_eq!(err.err().unwrap().code, ErrorCode::QuoteMismatch);
    // Допуск 100% и худшая цена так мала, что минимум округляется до 0 единиц — обмена нет.
    set(&c, "exec.dex_slippage_pct", json!(100)).await;
    let tiny = Decimal::new(1, 28);
    let err = trade::order(
        &c,
        &req(tiny),
        &e,
        Decimal::ONE,
        (Decimal::ONE, Decimal::ONE),
        (18, 6),
    );
    assert_eq!(err.err().unwrap().code, ErrorCode::QuoteMismatch);
    // Нормальная цена — минимум больше нуля.
    let o = trade::order(
        &c,
        &req(Decimal::ONE),
        &e,
        Decimal::ONE,
        (Decimal::ONE, Decimal::ONE),
        (18, 6),
    )
    .unwrap();
    assert_eq!(o.min_out, U256::from(1_000_000u32));
}

#[tokio::test]
async fn inflow_and_swap_do_not_overlap() {
    let c = ctx("inflow").await;
    set(&c, "transfer.max_time_s", json!(3600)).await;
    let f = Inflows::default();
    f.expect(USDT).await;
    // Ожидается поступление USDT — обмен с USDT на выходе ждёт.
    let waiting = tokio::time::timeout(Duration::from_millis(50), f.swap(&c, USDT)).await;
    assert!(waiting.is_err());
    // Другой токен не ждёт.
    drop(f.swap(&c, COIN).await.unwrap());
    // Зачислено — обмен идёт; повторный статус того же перевода не снимает чужое ожидание.
    let tx = alloy_primitives::B256::repeat_byte(7);
    f.done(USDT, tx);
    let guard = f.swap(&c, USDT).await.unwrap();
    // Пока идёт обмен, адрес депозита USDT ждёт.
    let dep = tokio::time::timeout(Duration::from_millis(50), f.expect(USDT)).await;
    assert!(dep.is_err());
    drop(guard);
    f.expect(USDT).await;
    f.done(USDT, tx);
    let still = tokio::time::timeout(Duration::from_millis(50), f.swap(&c, USDT)).await;
    assert!(
        still.is_err(),
        "повторный статус того же хеша не снимает новое ожидание"
    );
}
