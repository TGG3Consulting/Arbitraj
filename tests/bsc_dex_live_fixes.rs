//! Файл: tests/bsc_dex_live_fixes.rs
//! Что делает: проверки исправлений по живому запуску (журнал Тиграна, 24.10):
//! - разбор откатов квотера «не хватает ликвидности» на НАСТОЯЩИХ ответах узла BSC из журнала (Uniswap V4 и Infinity CL —
//!   UnexpectedRevertBytes(NotEnoughLiquidity), Infinity Bin — BinPool__OutOfLiquidity, QuoterV2 — «Unexpected error»);
//! - «Перепроверить» по одной монете кладёт в очереди циклов только её (а не все монеты подписки);
//! - пустой пул (резерв 0 / ликвидность 0) распознаётся по состоянию;
//! - расчёт V3 называет конкретный шаг, на котором он невозможен;
//! - темп узла HTTP (bsc.node_http_rps): вызовы идут не залпом;
//! - стартовая загрузка пулов и журнал «одна запись на вид причины».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex, arb::settings, arb::storage (настоящая SQLite во временной папке). Состояния
//! пулов и тики заданы числами (синтетика: узлы BSC из песочницы недоступны), ответы квотера — записанные.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256, hex};
use chrono::Utc;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::{Pool, PoolRef, PoolState, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::check::recheck::Recheck;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::math::tick_math::sqrt_ratio_at_tick;
use arb::venues::bsc_dex::node::pace;
use arb::venues::bsc_dex::pools::families::FAMILIES;
use arb::venues::bsc_dex::pools::progress::{Signals, reason_key};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::{emptiness, quoter_revert};

/// Ответ узла на eth_call квотера (как в журнале: код 3, текст и данные отката).
fn reverted(message: &str, data: &str) -> GatewayError {
    let raw =
        json!({"jsonrpc": "2.0", "id": 1, "error": {"code": 3, "message": message, "data": data}});
    GatewayError::new(
        ErrorCode::SwapReverted,
        None,
        format!("узел BSC (HTTP), код 3: {message}"),
    )
    .with_raw(raw.to_string())
}

#[test]
fn quoter_liquidity_reverts_from_live_journal_are_one_class() {
    // Uniswap V4 ETH, 11:10:11 — UnexpectedRevertBytes(NotEnoughLiquidity(poolId)).
    let v4 = "0x6190b2b0000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000247a5ed734577f715881f8da54e2d256f4496138a49ce7294eef19b19121af27925e3da7cd00000000000000000000000000000000000000000000000000000000";
    // PancakeSwap Infinity Bin ETH, 11:16:31 — UnexpectedRevertBytes(BinPool__OutOfLiquidity()).
    let bin = "0x6190b2b00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000496aa65ad00000000000000000000000000000000000000000000000000000000";
    let e = reverted("execution reverted", v4);
    assert_eq!(
        quoter_revert::liquidity_short(&e),
        Some("квотер: NotEnoughLiquidity")
    );
    let e = reverted("execution reverted", bin);
    assert_eq!(
        quoter_revert::liquidity_short(&e),
        Some("квотер: BinPool__OutOfLiquidity")
    );
    // PancakeSwap V3 ETH, 11:03:26 — QuoterV2 Error("Unexpected error").
    let unexpected = "0x08c379a000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000010556e6578706563746564206572726f7200000000000000000000000000000000";
    let e = reverted("execution reverted: Unexpected error", unexpected);
    assert!(quoter_revert::liquidity_short(&e).is_some());
    assert!(quoter_revert::classify("execution reverted: SPL", None).is_some());
    // Другой откат — не «не хватает ликвидности»: показывается как есть.
    assert!(quoter_revert::classify("execution reverted: STF", None).is_none());
    assert!(quoter_revert::from_data(&hex::decode(&unexpected[2..]).unwrap()).is_none());
    let other = GatewayError::new(ErrorCode::RateLimited, None, "429");
    assert!(quoter_revert::liquidity_short(&other).is_none());
}

#[test]
fn recheck_button_queues_only_given_coins() {
    let r = Recheck::default();
    r.coins(&["TOWNS".to_string()]);
    let (taxes, new_pools) = r.taxes.take();
    assert_eq!(
        taxes.into_iter().collect::<Vec<_>>(),
        vec!["TOWNS".to_string()]
    );
    assert!(!new_pools);
    assert!(r.taxes.take().0.is_empty(), "очередь забрана");
    assert_eq!(r.quoter.take().0.len(), 1);
    // Новые пулы V4 — только циклам пулов, налоги не трогаются (их будит «пул готов»).
    r.new_pools();
    assert!(r.hooks.take().1);
    assert!(!r.taxes.take().1);
}

/// Запись пула для проверки пустоты.
fn entry(state: PoolState) -> Entry {
    let family = FAMILIES.iter().find(|f| f.id == "pancakeswap_v3").unwrap();
    Entry {
        family,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: family.id.into(),
            kind: family.kind,
            reference: PoolRef::Contract(Address::ZERO),
            token0: Address::ZERO,
            token1: Address::ZERO,
            fee_pips: Some(500),
            tick_spacing: Some(10),
            bin_step: None,
            hooks: None,
            state,
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: Address::ZERO,
        usdt: Address::ZERO,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
    }
}

#[test]
fn empty_pool_is_detected_by_state() {
    let v3 = |liquidity| PoolState::V3 {
        sqrt_price_x96: U256::from(1u8),
        liquidity,
        tick: 0,
    };
    assert!(emptiness::empty(&entry(v3(0))));
    assert!(!emptiness::empty(&entry(v3(5))));
    let v2 = |r0: u8| PoolState::V2 {
        reserve0: U256::from(r0),
        reserve1: U256::from(7u8),
    };
    assert!(emptiness::empty(&entry(v2(0))));
    assert!(!emptiness::empty(&entry(v2(3))));
}

#[test]
fn v3_math_names_the_failing_step() {
    // Тик 0 инициализирован с net = +1000, а текущая ликвидность 100: при движении вниз (zeroForOne) пересечение
    // вычитает 1000 — карта тиков не сходится с ликвидностью пула.
    let mut ticks = BTreeMap::new();
    ticks.insert(
        0,
        TickData {
            gross: 1000,
            net: 1000,
        },
    );
    let map = TickMap {
        spacing: 1,
        ticks,
        words: (-1, 0),
    };
    let s = ClState {
        sqrt_price: sqrt_ratio_at_tick(0).unwrap(),
        tick: 0,
        liquidity: 100,
        fee: 500,
        ticks: &map,
        version: Version::V3,
    };
    let err = swap_cl::swap(&s, true, true, U256::from(10u8)).unwrap_err();
    assert!(err.contains("карта тиков не сходится"), "{err}");
}

/// Окружение на настоящей SQLite.
async fn ctx(name: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

#[tokio::test]
async fn node_calls_are_paced_by_setting() {
    let c = ctx("pace").await;
    // Не задан темп — запрос не уходит.
    assert_eq!(
        pace::wait(&c, 1).await.unwrap_err().code,
        ErrorCode::NotConfigured
    );
    c.settings
        .set("bsc.node_http_rps", Scope::Global, Some(json!(100)), "тест")
        .await
        .unwrap();
    let t0 = tokio::time::Instant::now();
    for _ in 0..5 {
        pace::wait(&c, 1).await.unwrap();
    }
    // 5 вызовов при 100 в секунду: последний уходит не раньше 40 мс после первого.
    assert!(t0.elapsed() >= std::time::Duration::from_millis(40));
    // Пачка из 10 вызовов занимает 0,1 с очереди.
    let t1 = tokio::time::Instant::now();
    pace::wait(&c, 10).await.unwrap();
    pace::wait(&c, 1).await.unwrap();
    assert!(t1.elapsed() >= std::time::Duration::from_millis(100));
}

#[tokio::test]
async fn start_signal_and_one_journal_record_per_reason() {
    let c = ctx("signals").await;
    let s = Signals::default();
    s.expect(2);
    s.loaded();
    let waited = tokio::time::timeout(std::time::Duration::from_millis(50), s.wait_start()).await;
    assert!(waited.is_err(), "одна пачка ещё грузится");
    s.loaded();
    tokio::time::timeout(std::time::Duration::from_millis(50), s.wait_start())
        .await
        .expect("стартовая загрузка закончена");
    // Одна запись на вид причины: блоки и числа не делают причину новой.
    for b in [100, 101, 102] {
        s.once(
            &c,
            Level::Warn,
            "venue.ws",
            format!("узел HTTP стоит на блоке {b}"),
        )
        .await;
    }
    let list = c.journal.list(100, None).await.unwrap();
    assert_eq!(list.iter().filter(|e| e.kind == "venue.ws").count(), 1);
    assert_eq!(
        reason_key("расхождение 61.4% > 0.1%"),
        "расхождение .% > .%"
    );
}
