//! Файл: tests/core_coin_reentry_t179.rs
//! Что делает: задача t179 (решение 390) на настоящем ядре и SQLite во временной папке (площадок в реестре нет, моков
//! нет): несколько сделок одной монеты, пока прошлая везёт монету.
//! - Замок (core/coin_ride.rs): money.max_deals_per_coin не задана — одна сделка на монету, отказ тем же текстом, что до
//!   t179 (start_reason::busy); задана 2 — вторая сделка того же направления начинается, когда первая учла покупку
//!   (шаг Transfer), третья — нет (предел); встречная (bsc_dex → bingx при идущей bingx → bsc_dex) — нет; идущая ушла в
//!   застрявшую — новых нет; ручное действие при идущей — отказ; конец сделки снимает только свою запись.
//! - Деньги (core/limits_parts.rs): вложенное идущей сделки вычитается из money.max_per_coin_usd и money.max_in_transit_usd
//!   у второй — двойного бюджета нет.
//! - Ликвидность продажи (core/ride_quote.rs marginal): вторая сделка считается по стакану / пулу ПОСЛЕ монет первой:
//!   биржа — записанные bids BingX CAKE-USDT (tests/data/bingx/ws_spot_2026-10-23.jsonl) и настоящая котировка по стакану
//!   (gateway::frozen::FrozenBook::price): разность котировок = продажа по стакану, из которого сняты монеты первой;
//!   DEX — записанный пул CAKE/USDT PancakeSwap V2 (резервы getReserves, tests/size_common) и математика адаптера
//!   (quote/pool_quote.rs swap): разность = продажа в пул после продажи первой (до округления формулы V2).
//! - Свежесть покупки (ride_quote.rs buy_fresh): стакан A не новее конца покупки идущей сделки — отказ; DEX: маршрут
//!   через пул покупки идущей по блоку раньше её блока — отказ, по её блоку — можно, через другой пул — можно.
//! - Пауза после удачной сделки (route_pause.rs won).
//!
//! Ставка комиссии тейкера в проверке разности — параметр проверки (в записях BingX ставки аккаунта нет,
//! tests/size_common/mod.rs), не данные биржи: разность должна совпасть при любой ставке.
//! Не покрыто здесь (нужны живые площадки): путь executor::accepted → board → release целиком, Live-источник расчёта
//! (costs_source.rs) с котировками шлюза, симуляция продажи DEX (precheck_dex_sell.rs) — см. docs/notes/t179-*.md.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::venues::bsc_dex (математика
//! пула), tests/size_common.

mod size_common;

use std::path::PathBuf;

use alloy_primitives::{Address, B256};
use arb::core::Core;
use arb::core::coin_key::CoinKey;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::dry::Mode;
use arb::core::types::{
    ChainTx, Cost, CostKind, DexRoute, OrderBook, PoolKind, PoolRef, PoolState, PriceLevel, Quote,
    RouteHop, RouteLeg, RouteMode, SendMethod, Side, TradeRequest, TradeResult, TradeStatus,
    TxOutcome, VenueId,
};
use arb::core::{coin_ride, limits_parts, ride_quote, route_pause, start_reason};
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use size_common::{CAKE, PAIR, Rec, d};
use tokio::sync::broadcast;

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-t179-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе во временной папке. Возвращает ядро и настройки.
async fn start(name: &str) -> (Core, Store) {
    let d = dir(name);
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
        data_dir: d.clone(),
    })
    .await;
    (core, settings)
}

/// Задать настройку всей системы. Ошибок нет (тест падает).
async fn set(s: &Store, name: &str, v: serde_json::Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Направление CAKE (записи tests/size_common): A → B.
fn dir_of(a: &str, b: &str) -> arb::core::types::Direction {
    let mut x = size_common::calc::direction();
    x.buy_on = VenueId(a.into());
    x.sell_on = VenueId(b.into());
    x
}

/// Сделка, учётшая покупку: шаг, вложено, монет; покупка — запрос с маршрутом и транзакцией (у биржи — None).
fn bought(
    id: &str,
    dir: &arb::core::types::Direction,
    step: Step,
    invested: &str,
    qty: &str,
) -> Deal {
    let mut x = Deal::new(id.into(), dir.clone(), json!({}), d("1"));
    x.step = step;
    x.invested = d(invested);
    x.qty = d(qty);
    x.buy = Some(trade(id, dir, None, None, Utc::now()));
    x
}

/// Исполнение покупки (статус Filled) с маршрутом DEX и блоком транзакции (None — биржа).
fn trade(
    id: &str,
    dir: &arb::core::types::Direction,
    route: Option<DexRoute>,
    block: Option<u64>,
    at: chrono::DateTime<Utc>,
) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: dir.buy_on.clone(),
            asset: dir.asset.clone(),
            side: Side::Buy,
            qty: d("10"),
            worst_price: d("3"),
            deal_id: id.into(),
            route,
            exit_position: false,
            remeasure: None,
        },
        status: TradeStatus::Filled,
        filled_qty: d("10"),
        avg_price: d("2"),
        fees: vec![],
        venue_ref: id.into(),
        chain: block.map(|b| ChainTx {
            hash: B256::ZERO,
            nonce: 0,
            channels: vec![],
            method: SendMethod::Single,
            outcome: TxOutcome::Included,
            block: Some(b),
            gas_used: None,
            gas_bnb: None,
            tip_bnb: None,
            cancel_hash: None,
            cancel_gas_bnb: None,
            approve_gas_bnb: None,
            tip_hash: None,
            tip_gas_bnb: None,
        }),
        finished_at: at,
        executor: None,
    }
}

/// Сохранить сделку в учёт и занять монету так, как это делает исполнитель (admit → board). Ошибок нет.
async fn ride(core: &Core, deal: &Deal) {
    coin_ride::admit(core, &deal.direction).unwrap();
    arb::core::ledger::save(core, deal).await;
    coin_ride::board(core, &deal.direction, &deal.id);
}

#[tokio::test]
async fn unset_keeps_one_deal_per_coin_with_old_text() {
    let (core, _s) = start("unset").await;
    let dx = dir_of("bingx", "bsc_dex");
    let key = CoinKey::of(&dx.asset);
    // До покупки — замок целиком: второй вход отказан, ручное действие тоже.
    coin_ride::admit(&core, &dx).unwrap();
    assert!(coin_ride::admit(&core, &dx).is_err());
    assert!(!coin_ride::take(&core, &key));
    coin_ride::release(&core, &key, None);
    assert!(!coin_ride::held(&core, &key));
    // Покупка учтена, сделка везёт монету: без настройки — одна сделка на монету, отказ прежним текстом.
    let first = bought("D1", &dx, Step::Transfer, "40", "10");
    ride(&core, &first).await;
    let why = coin_ride::admit(&core, &dx).unwrap_err();
    let old = start_reason::busy(core.inner.deals.lock().unwrap().values(), &dx);
    assert_eq!(why, old);
    assert!(why.starts_with("по монете идёт другая сделка D1"), "{why}");
    assert!(coin_ride::blocks_new(&core, &dx));
    assert!(coin_ride::held(&core, &key));
    assert!(
        !coin_ride::take(&core, &key),
        "ручное действие при идущей — отказ"
    );
    // Конец сделки снимает её запись — монета свободна.
    coin_ride::release(&core, &key, Some("D1"));
    assert!(!coin_ride::held(&core, &key));
    assert!(coin_ride::admit(&core, &dx).is_ok());
}

#[tokio::test]
async fn limit_two_second_deal_starts_third_waits() {
    let (core, s) = start("two").await;
    set(&s, coin_ride::MAX_DEALS, json!(2)).await;
    let dx = dir_of("bingx", "bsc_dex");
    let key = CoinKey::of(&dx.asset);
    let first = bought("D1", &dx, Step::Transfer, "40", "10");
    ride(&core, &first).await;
    assert!(!coin_ride::blocks_new(&core, &dx), "расчёт второй нужен");
    // Вторая: до своей покупки держит монету целиком — третья ждёт, ручных действий нет.
    coin_ride::admit(&core, &dx).unwrap();
    assert!(coin_ride::admit(&core, &dx).is_err());
    assert!(coin_ride::blocks_new(&core, &dx));
    // Вторая учла покупку — идущих 2 = предел: третьей нет.
    let second = bought("D2", &dx, Step::Transfer, "30", "8");
    arb::core::ledger::save(&core, &second).await;
    coin_ride::board(&core, &dx, "D2");
    let why = coin_ride::admit(&core, &dx).unwrap_err();
    assert!(why.contains("предел money.max_deals_per_coin = 2"), "{why}");
    // Первая кончилась — снимается только её запись; вторая идёт, третья может начаться.
    coin_ride::release(&core, &key, Some("D1"));
    assert_eq!(coin_ride::of_coin(&core, &key).len(), 1);
    assert!(coin_ride::admit(&core, &dx).is_ok());
    // Замок целиком у третьей; конец второй его не снимает.
    coin_ride::release(&core, &key, Some("D2"));
    assert!(
        coin_ride::admit(&core, &dx).is_err(),
        "замок третьей на месте"
    );
}

#[tokio::test]
async fn counter_and_off_path_rides_block_new_deals() {
    let (core, s) = start("counter").await;
    set(&s, coin_ride::MAX_DEALS, json!(3)).await;
    let dx = dir_of("bingx", "bsc_dex");
    let mut first = bought("D1", &dx, Step::Transfer, "40", "10");
    ride(&core, &first).await;
    // Встречная: купить на DEX, где идущая продаёт.
    let back = dir_of("bsc_dex", "bingx");
    let why = coin_ride::admit(&core, &back).unwrap_err();
    assert!(why.starts_with("встречная сделка: идёт D1"), "{why}");
    assert!(coin_ride::blocks_new(&core, &back));
    // Продать на бирже, где идущая покупала, — тоже встречная.
    assert!(coin_ride::admit(&core, &dir_of("bsc_dex", "bingx")).is_err());
    assert!(
        coin_ride::admit(&core, &dir_of("mexc", "bingx"))
            .unwrap_err()
            .starts_with("встречная")
    );
    // Другая биржа → та же DEX — не встречная.
    assert!(coin_ride::admit(&core, &dir_of("mexc", "bsc_dex")).is_ok());
    coin_ride::release(&core, &CoinKey::of(&dx.asset), None);
    // Идущая застряла — новых нет.
    first.step = Step::Stuck;
    arb::core::ledger::save(&core, &first).await;
    let why = coin_ride::admit(&core, &dx).unwrap_err();
    assert!(why.contains("не на пути «перевод / продажа»"), "{why}");
}

#[tokio::test]
async fn second_budget_excludes_first_invested() {
    let (core, s) = start("budget").await;
    set(&s, coin_ride::MAX_DEALS, json!(2)).await;
    set(&s, "money.max_per_trade_usd", json!("50")).await;
    set(&s, "money.max_per_coin_usd", json!("70")).await;
    set(&s, "money.max_in_transit_usd", json!("100")).await;
    let dx = dir_of("bingx", "bsc_dex");
    let before = limits_parts::parts(&core, &dx, &Mode::Live).unwrap();
    assert_eq!(before.cap(), d("50"));
    ride(&core, &bought("D1", &dx, Step::Transfer, "40", "10")).await;
    let p = limits_parts::parts(&core, &dx, &Mode::Live).unwrap();
    assert_eq!(p.coin_left, d("30"), "70 − вложено первой 40");
    assert_eq!(p.transit_left, d("60"), "100 − в пути 40");
    assert_eq!(p.cap(), d("30"));
    assert_eq!(p.binding(), "per_coin");
}

/// Стакан BingX CAKE-USDT из записи; bids без первых `off` монет (их продала первая сделка).
fn book(rec: &Rec, off: Decimal) -> OrderBook {
    let mut left = off;
    let mut bids = vec![];
    for l in &rec.bids {
        let take = left.min(l.qty);
        left -= take;
        if l.qty > take {
            bids.push(PriceLevel {
                price: l.price,
                qty: l.qty - take,
            });
        }
    }
    OrderBook {
        venue: VenueId("bingx".into()),
        asset: size_common::calc::direction().asset,
        bids,
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    }
}

/// Котировка продажи по стакану (настоящая формула gateway::frozen), ставка — параметр проверки.
fn sell_quote(b: OrderBook, qty: Decimal) -> Quote {
    FrozenBook {
        venue: b.venue.clone(),
        asset: b.asset.clone(),
        side: Side::Sell,
        fee: BookFee::in_received(d("0.001"), Side::Sell, "CAKE", "USDT"),
        taken_at: Utc::now(),
        book: b,
    }
    .price(qty)
}

#[tokio::test]
async fn cex_sell_counts_book_after_first_deal_coins() {
    let rec = Rec::load("t179-cex").await;
    let depth: Decimal = rec.bids.iter().map(|l| l.qty).sum();
    // Округлены до 4 знаков: разность (off + qty) − off в Decimal — ровно qty.
    let (off, qty) = ((depth / d("3")).round_dp(4), (depth / d("4")).round_dp(4));
    let full = book(&rec, Decimal::ZERO);
    let m = ride_quote::marginal(
        &sell_quote(full.clone(), off + qty),
        &sell_quote(full.clone(), off),
        qty,
        off,
    );
    let seq = sell_quote(book(&rec, off), qty);
    assert_eq!(m.fillable_qty, seq.fillable_qty);
    let (mu, su) = (
        m.avg_price * m.fillable_qty,
        seq.avg_price * seq.fillable_qty,
    );
    assert!((mu - su).abs() <= su * d("0.000000000001"), "{mu} vs {su}");
    assert!(
        m.avg_price < sell_quote(full.clone(), qty).avg_price,
        "после первой — хуже, чем без неё"
    );
    let fee = |q: &Quote| {
        q.costs
            .iter()
            .find(|c| c.kind == CostKind::TradeFee)
            .unwrap()
            .amount
    };
    assert!((fee(&m) - fee(&seq)).abs() <= fee(&seq) * d("0.000000000001"));
    // Глубины не хватает на монеты первой и наши — исполнимо только остаток.
    let m2 = ride_quote::marginal(
        &sell_quote(full.clone(), depth + qty),
        &sell_quote(full, off),
        qty + depth,
        off,
    );
    assert_eq!(m2.fillable_qty, depth - off);
}

#[tokio::test]
async fn dex_sell_counts_pool_after_first_deal_coins() {
    let rec = Rec::load("t179-dex").await;
    let units = |x: Decimal| to_units(x, 18).unwrap();
    let out = |e: &arb::venues::bsc_dex::pools::registry::Entry, x: Decimal| {
        to_decimal(
            swap(&rec.ctx, e, CAKE, true, units(x)).unwrap().amount_out,
            18,
        )
        .unwrap()
    };
    let (off, qty) = (d("10"), d("5"));
    let q = |x: Decimal, usd: Decimal, gas: &str| Quote {
        venue: VenueId("bsc_dex".into()),
        asset: size_common::calc::direction().asset,
        side: Side::Sell,
        qty: x,
        fillable_qty: x,
        avg_price: usd / x,
        costs: vec![
            Cost::new(CostKind::Gas, d(gas), "BNB", false),
            Cost::new(CostKind::PoolFee, x * d("0.0025"), "CAKE", true),
        ],
        route: None,
        signal_route: None,
        pick: None,
        at: Utc::now(),
    };
    let (t, a) = (out(&rec.pool, off + qty), out(&rec.pool, off));
    let m = ride_quote::marginal(&q(off + qty, t, "0.0002"), &q(off, a, "0.0002"), qty, off);
    // Пул после продажи первой: резерв CAKE + off, резерв USDT − её выход.
    let mut after = rec.pool.clone();
    let PoolState::V2 { reserve0, reserve1 } = after.pool.state.clone() else {
        panic!("V2")
    };
    after.pool.state = PoolState::V2 {
        reserve0: reserve0 + units(off),
        reserve1: reserve1 - units(a),
    };
    let seq = out(&after, qty);
    let mine = m.avg_price * m.fillable_qty;
    assert_eq!(m.fillable_qty, qty);
    // Формула V2 с комиссией: разность и последовательная продажа совпадают до ~1e-7 доли (округление формулы).
    assert!((mine - seq).abs() <= seq * d("0.000001"), "{mine} vs {seq}");
    assert!(
        mine < out(&rec.pool, qty),
        "после первой — меньше, чем в нетронутый пул"
    );
    let gas = m
        .costs
        .iter()
        .find(|c| c.kind == CostKind::Gas)
        .unwrap()
        .amount;
    assert_eq!(gas, d("0.0002"), "газ — одного нашего обмена");
    let fee = m
        .costs
        .iter()
        .find(|c| c.kind == CostKind::PoolFee)
        .unwrap()
        .amount;
    assert_eq!(fee, qty * d("0.0025"), "комиссия пула — нашей части");
}

/// Маршрут DEX через пул по блоку.
fn route(pool: Address, block: u64) -> DexRoute {
    DexRoute {
        mode: RouteMode::Direct,
        legs: vec![RouteLeg {
            share: Decimal::ONE,
            hops: vec![RouteHop {
                family: "pancakeswap_v2".into(),
                kind: PoolKind::V2,
                pool: PoolRef::Contract(pool),
            }],
            expected_out: d("10"),
        }],
        block,
        gas_units: 0,
        executable: true,
        exec_note: None,
    }
}

#[tokio::test]
async fn buy_needs_data_newer_than_running_buy() {
    let (core, s) = start("fresh").await;
    set(&s, coin_ride::MAX_DEALS, json!(3)).await;
    let dx = dir_of("bingx", "bsc_dex");
    let t = Utc::now();
    let mut first = bought("D1", &dx, Step::Transfer, "40", "10");
    first.buy = Some(trade("D1", &dx, None, None, t));
    ride(&core, &first).await;
    let mut q = size_common::calc::calc(&Rec::load("t179-fresh").await, d("5")).buy_quote;
    q.at = t - Duration::milliseconds(1);
    assert!(
        ride_quote::buy_fresh(&core, &dx, &q)
            .unwrap_err()
            .contains("ждём кадр после неё")
    );
    q.at = t + Duration::milliseconds(1);
    assert!(ride_quote::buy_fresh(&core, &dx, &q).is_ok());
    // Покупка на DEX: идущая купила через пул PAIR в блоке 100.
    let dd = dir_of("bsc_dex", "mexc");
    let mut dex = bought("D2", &dd, Step::Transfer, "20", "5");
    dex.buy = Some(trade("D2", &dd, Some(route(PAIR, 100)), Some(100), t));
    arb::core::ledger::save(&core, &dex).await;
    coin_ride::board(&core, &dd, "D2");
    q.route = Some(route(PAIR, 99));
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q)
            .unwrap_err()
            .contains("раньше блока 100")
    );
    q.route = Some(route(PAIR, 100));
    assert!(ride_quote::buy_fresh(&core, &dd, &q).is_ok());
    q.route = Some(route(Address::repeat_byte(7), 1));
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q).is_ok(),
        "другой пул — проверки нет"
    );
    // Монеты идущих сделок, которые будут проданы на bsc_dex: только D1 (D2 продаёт на mexc).
    assert_eq!(
        ride_quote::sell_ahead(&core, &dir_of("mexc", "bsc_dex")),
        d("10")
    );
}

#[test]
fn won_deal_skips_direction_pause() {
    let dx = dir_of("bingx", "bsc_dex");
    let mut x = bought("D1", &dx, Step::Closed, "40", "0");
    x.at = dx.sell_on.clone();
    x.sells.push(trade("D1", &dx, None, None, Utc::now()));
    x.result_usd = Some(d("0.5"));
    assert!(route_pause::won(&x));
    x.result_usd = Some(d("-0.01"));
    assert!(!route_pause::won(&x), "убыток — пауза");
    x.result_usd = Some(d("0.5"));
    x.relays = 1;
    assert!(!route_pause::won(&x), "перегон — пауза");
    x.relays = 0;
    x.sells.clear();
    assert!(!route_pause::won(&x), "без продажи бота — пауза");
}
