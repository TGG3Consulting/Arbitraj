//! Файл: tests/bsc_dex_route_calc.rs
//! Что делает: проверяет общий расчёт маршрута из нескольких пулов (quote/route_calc.rs, решение 80) — путь глубины 3
//! «USDT → A → B → монета» по трём парам V2: порядок хопов, итог покупки и продажи, цена рядом с серединой рынка.
//! Двухшаговый путь (глубина 2) уже сквозно проверен в tests/bsc_dex_via.rs — он теперь идёт через этот же расчёт,
//! так что тот тест сторожит эквивалентность старого поведения.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{route_calc, select}, arb::settings (SQLite во временной папке).
//! Резервы пар заданы числами — записанных пулов BSC нет (из песочницы узлы BSC недоступны); проверяется композиция
//! хопов, а не рынок (формула пары getAmountOut/getAmountIn проверена на значениях v2-periphery в tests/bsc_dex_math.rs).

use std::str::FromStr;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::broadcast;

use arb::core::types::{Pool, PoolRef, PoolState, Side, VenueId};
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::route_calc::{self, Leg};
use arb::venues::bsc_dex::quote::select::Usable;

const USDT: Address = address!("0x1000000000000000000000000000000000000001");
const A: Address = address!("0x2000000000000000000000000000000000000002");
const B: Address = address!("0x3000000000000000000000000000000000000003");
const COIN: Address = address!("0x4000000000000000000000000000000000000004");

/// Минимальное окружение: расчёту нужны только Ctx (для ошибок) и пулы (комиссия пары — своя, fee_pips).
async fn ctx() -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-route-calc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = arb::settings::store::Store::load(db, changes, journal.clone())
        .await
        .unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Пара V2 с резервами и своей комиссией 0,25 % (fee_pips = 2500 → 0,25 %). token0 < token1 по адресу.
fn pair(addr: Address, x: Address, y: Address, rx: u128, ry: u128) -> Entry {
    let f = families::make("pancakeswap_v2", "pancakeswap_v2").unwrap();
    let (t0, t1, r0, r1) = if x < y {
        (x, y, rx, ry)
    } else {
        (y, x, ry, rx)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr),
            token0: t0,
            token1: t1,
            fee_pips: Some(2500),
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::from(r0),
                reserve1: U256::from(r1),
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: x,
        usdt: y,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// Пул как хоп маршрута без крючка и с измеренным газом.
fn leg(e: &Entry, token_in: Address, token_out: Address) -> Leg {
    Leg {
        entry: e.clone(),
        usable: Usable {
            gas: 100_000,
            hook_pct: Decimal::ZERO,
        },
        token_in,
        token_out,
        dec_in: 18,
        dec_out: 18,
    }
}

#[tokio::test]
async fn three_hops_buy_and_sell() {
    let ctx = ctx().await;
    let e18 = 10u128.pow(18);
    // Каждая пара 1:1 и глубокая: цена монеты ≈ 1 USDT; три комиссии 0,25 % ⇒ покупка чуть дороже, продажа дешевле.
    let usdt_a = pair(
        address!("0x00000000000000000000000000000000000000a1"),
        USDT,
        A,
        1_000_000 * e18,
        1_000_000 * e18,
    );
    let a_b = pair(
        address!("0x00000000000000000000000000000000000000a2"),
        A,
        B,
        1_000_000 * e18,
        1_000_000 * e18,
    );
    let b_coin = pair(
        address!("0x00000000000000000000000000000000000000a3"),
        B,
        COIN,
        1_000_000 * e18,
        1_000_000 * e18,
    );
    let qty = Decimal::from(10u8);
    let one = Decimal::ONE;

    // Покупка: USDT → A → B → монета.
    let buy = route_calc::eval(
        &ctx,
        &[
            leg(&usdt_a, USDT, A),
            leg(&a_b, A, B),
            leg(&b_coin, B, COIN),
        ],
        Side::Buy,
        qty,
        one,
    )
    .expect("путь покупки считается");
    let tokens: Vec<_> = buy.hops.iter().map(|h| (h.token_in, h.token_out)).collect();
    assert_eq!(tokens, vec![(USDT, A), (A, B), (B, COIN)]);
    assert!(
        buy.coins >= qty - Decimal::new(1, 12) && buy.coins <= qty,
        "{}",
        buy.coins
    );
    let buy_price = buy.usdt / buy.coins;
    assert!(
        buy_price > one && buy_price < Decimal::from_str("1.02").unwrap(),
        "покупка дороже середины рынка, но близко: {buy_price}"
    );

    // Продажа: монета → B → A → USDT.
    let sell = route_calc::eval(
        &ctx,
        &[
            leg(&b_coin, COIN, B),
            leg(&a_b, B, A),
            leg(&usdt_a, A, USDT),
        ],
        Side::Sell,
        qty,
        one,
    )
    .expect("путь продажи считается");
    let tokens: Vec<_> = sell
        .hops
        .iter()
        .map(|h| (h.token_in, h.token_out))
        .collect();
    assert_eq!(tokens, vec![(COIN, B), (B, A), (A, USDT)]);
    assert_eq!(sell.coins, qty);
    let sell_price = sell.usdt / sell.coins;
    assert!(
        sell_price < one && sell_price > Decimal::from_str("0.98").unwrap(),
        "продажа дешевле середины рынка, но близко: {sell_price}"
    );

    // Покупка дороже продажи — на суммарную комиссию трёх пулов.
    assert!(buy_price > sell_price);
}
