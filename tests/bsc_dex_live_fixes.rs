//! Файл: tests/bsc_dex_live_fixes.rs
//! Что делает: проверки исправлений по живому запуску (журнал Тиграна, 24.10):
//! - разбор откатов квотера «не хватает ликвидности» на НАСТОЯЩИХ ответах узла BSC из журнала (Uniswap V4 и Infinity CL —
//!   UnexpectedRevertBytes(NotEnoughLiquidity), Infinity Bin — BinPool__OutOfLiquidity, QuoterV2 — «Unexpected error»);
//! - «Перепроверить» по одной монете кладёт в очереди циклов только её (а не все монеты подписки);
//! - пустой пул (резерв 0 / ликвидность 0) распознаётся по состоянию;
//! - расчёт V3 называет конкретный шаг, на котором он невозможен;
//!
//! Темп узла и сигналы подписки — tests/bsc_dex_live_node.rs.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex, arb::settings, arb::storage (настоящая SQLite во временной папке). Состояния
//! пулов и тики заданы числами (синтетика: узлы BSC из песочницы недоступны), ответы квотера — записанные.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256, hex};
use chrono::Utc;
use serde_json::json;

use arb::core::types::{Pool, PoolRef, PoolState, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::check::recheck::Recheck;
use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::math::tick_math::sqrt_ratio_at_tick;
use arb::venues::bsc_dex::pools::families;
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
    let family = families::make("pancakeswap_v3", "v3_fork_pancake").unwrap();
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
        asleep: false,
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
