//! Файл: tests/core_coin_reentry_t179.rs
//! Что делает: решение 390 (t179) — докупка (записка договорённостей 269) на настоящем ядре и SQLite во временной папке
//! (площадок в реестре нет, моков нет):
//! - п. 15: exec.top_up не задана — одна сделка на монету, отказ прежним текстом (start_reason::busy), у сделки нет
//!   группы (JSON сделки без поля group), после удачной сделки пауза направления ставится как раньше (попытка паузы —
//!   Warn core.ledger о signal.route_cooldown_s), строки «Пауза не поставлена» нет; событие баланса без хеша при двух
//!   ожиданиях закрывает подходящее по сумме, как раньше;
//! - п. 1, 3: включена — вторая сделка ТОГО ЖЕ направления присоединяется к группе первой, когда первая учла покупку;
//!   другие направления монеты, застрявшая сделка группы, замок целиком — прежний отказ; ручное действие — отказ;
//! - деньги: бюджет докупки = лимит монеты − вложенное идущих (limits_parts.rs);
//! - п. 4: продажа докупки — разность котировок на записанных bids BingX CAKE-USDT (gateway::frozen::FrozenBook::price)
//!   и пуле CAKE/USDT V2 (математика адаптера) против последовательной продажи; покупка — данные новее покупки идущей
//!   (стакан / блок; агрегатор без маршрута — отказ);
//! - след продажи (ride_trail.rs): стакан не новее продажи сделки группы — отказ, новее — след снят;
//! - пп. 8, Б2: включена — событие без хеша при двух ожиданиях ничего не закрывает (сделка 1 ждёт 100, сделка 2 — 95, у
//!   первой дошло 95), только будит опрос;
//! - пп. 10, 11, 13: общий возврат USDT группы — первая закрытая ждёт (возврат 0), последняя перегоняет общую сумму,
//!   у отложенной — «общий возврат в сделке …»; последняя идущая застряла — отложенные уходят одним возвратом сразу;
//! - пауза после удачной сделки группы (route_pause.rs won).
//!
//! Ставка комиссии тейкера в проверке разности — параметр проверки (в записях BingX ставки аккаунта нет,
//! tests/size_common/mod.rs), не данные биржи. Не покрыто здесь (нужны живые площадки): путь executor::accepted → board
//! → release целиком, Live-источник расчёта с котировками шлюза, симуляция продажи DEX, сам перегон USDT.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::venues::bsc_dex (математика
//! пула), tests/size_common.

mod size_common;

use std::path::PathBuf;
use std::time::Duration as StdDuration;

use alloy_primitives::{Address, B256};
use arb::core::Core;
use arb::core::coin_key::CoinKey;
use arb::core::coin_ride::Seat;
use arb::core::deal::{Deal, Step};
use arb::core::deal_group::DealGroup;
use arb::core::deps::Deps;
use arb::core::dry::Mode;
use arb::core::types::{
    Asset, BalanceChange, BalanceKind, ChainTx, Cost, CostKind, DexRoute, OrderBook, PoolKind,
    PoolRef, PoolState, PriceLevel, Quote, RouteHop, RouteLeg, RouteMode, SendMethod, Side,
    TradeRequest, TradeResult, TradeStatus, TxOutcome, VenueId,
};
use arb::core::{
    coin_group, coin_ride, credit_event, credit_watch, limits_parts, ride_quote, ride_trail,
    route_pause, start_reason, transfer_credit,
};
use arb::gateway::contract::VenueKind;
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
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

/// Ядро на базе во временной папке; площадки bingx, mexc — биржи, bsc_dex — DEX (вид площадки для проверок свежести).
/// Возвращает ядро, настройки, журнал.
async fn start(name: &str) -> (Core, Store, Journal) {
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
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (VenueId("mexc".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    (core, settings, journal)
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

/// Сделка, учётшая покупку: шаг, вложено, монет; покупка — запрос с маршрутом и транзакцией (у биржи — None); группа
/// докупки (None — без докупки).
fn bought(
    id: &str,
    dir: &arb::core::types::Direction,
    step: Step,
    (invested, qty): (&str, &str),
    group: Option<&str>,
) -> Deal {
    let mut x = Deal::new(id.into(), dir.clone(), json!({}), d("1"));
    x.group = group.map(|g| Box::new(DealGroup::new(g)));
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

/// Сохранить сделку в учёт и занять монету так, как это делает исполнитель (admit → board). Возвращает место.
async fn ride(core: &Core, deal: &Deal) -> Seat {
    let seat = coin_ride::admit(core, &deal.direction).unwrap();
    arb::core::ledger::save(core, deal).await;
    let g = deal.group.as_ref().map(|g| g.id.clone());
    coin_ride::board(core, &deal.direction, &deal.id, g);
    seat
}

/// Включить докупку (exec.top_up = да). Ошибок нет (тест падает).
async fn top_up(s: &Store) {
    set(s, coin_ride::TOP_UP, json!(true)).await;
}

#[tokio::test]
async fn off_keeps_one_deal_per_coin_with_old_text() {
    let (core, _s, _j) = start("off").await;
    let dx = dir_of("bingx", "bsc_dex");
    let key = CoinKey::of(&dx.asset);
    // До покупки — замок целиком: второй вход отказан, ручное действие тоже.
    assert_eq!(coin_ride::admit(&core, &dx), Ok(Seat::Alone));
    assert!(coin_ride::admit(&core, &dx).is_err());
    assert!(!coin_ride::take(&core, &key));
    coin_ride::release(&core, &key, None);
    assert!(!coin_ride::held(&core, &key));
    // Покупка учтена, сделка везёт монету: одна сделка на монету, отказ прежним текстом.
    let first = bought("D1", &dx, Step::Transfer, ("40", "10"), None);
    assert_eq!(ride(&core, &first).await, Seat::Alone);
    let why = coin_ride::admit(&core, &dx).unwrap_err();
    let old = start_reason::busy(core.inner.deals.lock().unwrap().values(), &dx);
    assert_eq!(why, old);
    assert!(why.starts_with("по монете идёт другая сделка D1"), "{why}");
    assert!(coin_ride::blocks_new(&core, &dx));
    assert!(
        !coin_ride::take(&core, &key),
        "ручное действие при идущей — отказ"
    );
    // Сделка без группы — в JSON поля group нет (запись и API как раньше).
    let v = serde_json::to_value(&first).unwrap();
    assert!(v.get("group").is_none());
    coin_ride::release(&core, &key, Some("D1"));
    assert!(!coin_ride::held(&core, &key));
    assert!(coin_ride::admit(&core, &dx).is_ok());
}

#[tokio::test]
async fn off_successful_close_still_pauses_direction() {
    let (core, _s, j) = start("off-pause").await;
    let dx = dir_of("bingx", "bsc_dex");
    for (id, group) in [("P1", None), ("P2", Some("P2"))] {
        let mut x = bought(id, &dx, Step::Selling, ("40", "0"), group);
        x.at = dx.sell_on.clone();
        x.proceeds = d("41");
        x.sells.push(trade(id, &dx, None, None, Utc::now()));
        arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано").await;
        let won_line = x.timeline.iter().any(|(_, t)| t == route_pause::WON_NOTE);
        let warned = j.list(500, None).await.unwrap().iter().any(|e| {
            e.kind == "core.ledger"
                && e.level == Level::Warn
                && e.message.contains("signal.route_cooldown_s")
        });
        if group.is_none() {
            // Без группы (докупка выключена): попытка паузы, как раньше (signal.route_cooldown_s не задана — Warn).
            assert!(!won_line);
            assert!(warned, "пауза после удачной сделки — как раньше");
        } else {
            assert!(won_line, "у удачной сделки группы паузы нет");
        }
    }
}

#[tokio::test]
async fn on_same_direction_joins_other_directions_wait() {
    let (core, s, _j) = start("join").await;
    top_up(&s).await;
    let dx = dir_of("bingx", "bsc_dex");
    let key = CoinKey::of(&dx.asset);
    let mut first = bought("D1", &dx, Step::Transfer, ("40", "10"), Some("D1"));
    assert_eq!(ride(&core, &first).await, Seat::Lead);
    // Тот же направление — докупка в группу D1; до её покупки монета занята целиком.
    assert!(!coin_ride::blocks_new(&core, &dx));
    assert_eq!(coin_ride::admit(&core, &dx), Ok(Seat::Join("D1".into())));
    assert!(coin_ride::admit(&core, &dx).is_err());
    coin_ride::release(&core, &key, None);
    // Другие направления монеты — монета занята (прежний текст).
    for other in [
        dir_of("bsc_dex", "bingx"),
        dir_of("mexc", "bsc_dex"),
        dir_of("bingx", "mexc"),
    ] {
        let why = coin_ride::admit(&core, &other).unwrap_err();
        assert!(why.starts_with("по монете идёт другая сделка D1"), "{why}");
        assert!(coin_ride::blocks_new(&core, &other));
    }
    assert!(
        !coin_ride::take(&core, &key),
        "ручное действие при идущей — отказ"
    );
    // Сделка группы застряла — докупок нет.
    first.step = Step::Stuck;
    arb::core::ledger::save(&core, &first).await;
    assert!(coin_ride::admit(&core, &dx).is_err());
    assert!(coin_ride::blocks_new(&core, &dx));
    // Сделка без группы (начата до включения) — докупок нет.
    coin_ride::release(&core, &key, Some("D1"));
    let old = bought("D0", &dx, Step::Transfer, ("40", "10"), None);
    arb::core::ledger::save(&core, &old).await;
    coin_ride::board(&core, &dx, "D0", None);
    assert!(coin_ride::admit(&core, &dx).is_err());
}

#[tokio::test]
async fn top_up_budget_excludes_first_invested() {
    let (core, s, _j) = start("budget").await;
    top_up(&s).await;
    set(&s, "money.max_per_trade_usd", json!("50")).await;
    set(&s, "money.max_per_coin_usd", json!("70")).await;
    set(&s, "money.max_in_transit_usd", json!("100")).await;
    let dx = dir_of("bingx", "bsc_dex");
    let before = limits_parts::parts(&core, &dx, &Mode::Live).unwrap();
    assert_eq!(before.cap(), d("50"));
    ride(
        &core,
        &bought("D1", &dx, Step::Transfer, ("40", "10"), Some("D1")),
    )
    .await;
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
async fn top_up_buy_needs_data_newer_than_group_buy() {
    let (core, s, _j) = start("fresh").await;
    top_up(&s).await;
    let dx = dir_of("bingx", "bsc_dex");
    let t = Utc::now();
    let mut first = bought("D1", &dx, Step::Transfer, ("40", "10"), Some("D1"));
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
    // Покупка на DEX: сделка группы купила через пул PAIR в блоке 100.
    let dd = dir_of("bsc_dex", "mexc");
    let mut dex = bought("D2", &dd, Step::Transfer, ("20", "5"), Some("D2"));
    dex.buy = Some(trade("D2", &dd, Some(route(PAIR, 100)), Some(100), t));
    arb::core::ledger::save(&core, &dex).await;
    coin_ride::board(&core, &dd, "D2", Some("D2".into()));
    q.route = Some(route(PAIR, 99));
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q)
            .unwrap_err()
            .contains("её покупка в блоке 100")
    );
    q.route = Some(route(PAIR, 100));
    assert!(ride_quote::buy_fresh(&core, &dd, &q).is_ok());
    q.route = Some(route(Address::repeat_byte(7), 1));
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q).is_ok(),
        "другой пул — проверки нет"
    );
    q.route = None;
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q)
            .unwrap_err()
            .contains("агрегатор")
    );
    // Монеты идущих сделок группы, которые будут проданы на bsc_dex: только D1 (D2 продаёт на mexc).
    assert_eq!(
        ride_quote::sell_ahead(&core, &dir_of("mexc", "bsc_dex")),
        d("10")
    );
}

#[tokio::test]
async fn off_no_group_rides_change_nothing() {
    let (core, _s, _j) = start("off-quotes").await;
    let dx = dir_of("bingx", "bsc_dex");
    let mut first = bought("D1", &dx, Step::Transfer, ("40", "10"), None);
    first.buy = Some(trade("D1", &dx, None, None, Utc::now()));
    ride(&core, &first).await;
    let mut q = size_common::calc::calc(&Rec::load("t179-off").await, d("5")).buy_quote;
    q.at = Utc::now() - Duration::hours(1);
    // Идущая сделка без группы (теневой замер 267 и т. п.) — котировки как раньше.
    assert!(ride_quote::buy_fresh(&core, &dx, &q).is_ok());
    assert_eq!(ride_quote::sell_ahead(&core, &dx), Decimal::ZERO);
}

#[tokio::test]
async fn trail_waits_for_book_after_group_sale() {
    let (core, s, _j) = start("trail").await;
    top_up(&s).await;
    let dx = dir_of("bsc_dex", "bingx");
    let t = Utc::now();
    let mut x = bought("D1", &dx, Step::Closed, ("40", "0"), Some("D1"));
    let mut sale = trade("D1", &dx, None, None, t);
    sale.request.venue = dx.sell_on.clone();
    sale.request.side = Side::Sell;
    x.sells.push(sale);
    arb::core::ledger::save(&core, &x).await;
    let key = CoinKey::of(&dx.asset);
    let mut q = size_common::calc::calc(&Rec::load("t179-trail").await, d("5")).sell_quote;
    q.at = t;
    assert!(
        ride_trail::check(&core, &key, &dx.sell_on, &q)
            .unwrap_err()
            .contains("продажи сделки D1")
    );
    q.at = t + Duration::milliseconds(1);
    assert!(ride_trail::check(&core, &key, &dx.sell_on, &q).is_ok());
    q.at = t - Duration::seconds(1);
    assert!(
        ride_trail::check(&core, &key, &dx.sell_on, &q).is_ok(),
        "след снят"
    );
}

/// USDT кошелька bsc_dex (адрес — записи tests/size_common).
fn usdt() -> Asset {
    Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(size_common::USDT),
    }
}

/// Поступление без хеша (как поток BingX): сумма.
fn no_hash(amount: &str) -> BalanceChange {
    BalanceChange {
        venue: VenueId("bsc_dex".into()),
        asset: usdt(),
        delta: d(amount),
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: None,
        at: Utc::now(),
        source: "проверка t179".into(),
        account: None,
    }
}

/// Сценарий ревью Б2: сделка 1 ждёт 100, сделка 2 — 95, у первой дошло 95 (без хеша). Возвращает: закрыто ли у
/// сделки 2 и было ли событие у какой-либо.
async fn two_expectations(on: bool) -> (bool, bool) {
    let (core, s, _j) = start(if on { "credit-on" } else { "credit-off" }).await;
    if on {
        top_up(&s).await;
    }
    let v = Scope::Venue("bsc_dex".into());
    s.set(
        "transfer.credit_amount_tolerance_pct",
        v.clone(),
        Some(json!("0")),
        "тест",
    )
    .await
    .unwrap();
    s.set("transfer.credit_by_event", v, Some(json!(true)), "тест")
        .await
        .unwrap();
    let from = VenueId("bingx".into());
    let to = VenueId("bsc_dex".into());
    let t1 = credit_watch::register(&core, "D1", &from, &to, &usdt(), d("100"));
    let t2 = credit_watch::register(&core, "D2", &from, &to, &usdt(), d("95"));
    credit_event::on_change(&core, no_hash("95")).await;
    if on {
        tokio::time::timeout(StdDuration::from_millis(10), t1.wake.notified())
            .await
            .expect("опрос сделки 1 разбужен");
    }
    (
        transfer_credit::by_event(Some(&t2)).is_some(),
        t1.has_event() || t2.has_event(),
    )
}

#[tokio::test]
async fn credit_without_hash_two_expectations() {
    // Выключено — как раньше: подходящее по сумме ожидание (сделка 2) закрывается событием.
    assert_eq!(two_expectations(false).await, (true, true));
    // Включено — событие без хеша ни к одному не прикреплено, решает опрос по хешу.
    assert_eq!(two_expectations(true).await, (false, false));
}

/// Закрытая обычной продажей на площадке плана сделка группы `g` (выручка на sell_on). Ошибок нет.
fn sold(id: &str, dir: &arb::core::types::Direction, g: &str) -> Deal {
    let mut x = bought(id, dir, Step::Closed, ("40", "0"), Some(g));
    x.at = dir.sell_on.clone();
    x.result_usd = Some(d("1"));
    x.sells.push(trade(id, dir, None, None, Utc::now()));
    x
}

/// Сделки группы `g` на шаге продажи, учтённые в идущих (как после board). Ошибок нет.
async fn riding(core: &Core, dx: &arb::core::types::Direction, g: &str, ids: &[&str]) {
    for id in ids {
        let x = bought(id, dx, Step::Selling, ("40", "10"), Some(g));
        arb::core::ledger::save(core, &x).await;
        coin_ride::board(core, dx, id, Some(g.into()));
    }
}

/// Ждёт строку в хронологии сделки (общий возврат — отдельной задачей). Возвращает: сделку. Тест падает по сроку.
async fn wait_note(core: &Core, id: &str, part: &str) -> Deal {
    for _ in 0..200 {
        let d = core.inner.deals.lock().unwrap().get(id).cloned().unwrap();
        if d.timeline.iter().any(|(_, t)| t.contains(part)) {
            return d;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("нет строки «{part}» у {id}");
}

#[tokio::test]
async fn group_return_once_lead_last() {
    let (core, s, _j) = start("group-last").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    let b = dx.sell_on.clone();
    riding(&core, &dx, "G", &["G", "D2"]).await;
    // Вложенная закрылась первой — её возврат отложен (0 сейчас), носитель ещё неизвестен.
    let mut x = sold("D2", &dx, "G");
    let now = coin_group::route(&core, &mut x, vec![(b.clone(), d("95"))]).await;
    assert!(now.is_empty());
    assert_eq!(x.group.as_ref().unwrap().wait, Some((b.clone(), d("95"))));
    arb::core::ledger::save(&core, &x).await;
    // Первая — последняя: один возврат на общую сумму, отложенная помечена «общий возврат в сделке G».
    let mut g = sold("G", &dx, "G");
    let now = coin_group::route(&core, &mut g, vec![(b.clone(), d("100"))]).await;
    assert_eq!(now, vec![(b.clone(), d("195"))]);
    let sum = g.group.as_ref().unwrap().sum.clone().unwrap();
    assert_eq!(
        (sum.deals, sum.others, sum.usdt),
        (vec!["D2".to_string()], d("1"), d("95"))
    );
    let x = core.inner.deals.lock().unwrap().get("D2").cloned().unwrap();
    assert_eq!(x.group.as_ref().unwrap().shared_in.as_deref(), Some("G"));
    assert!(x.group.as_ref().unwrap().wait.is_none());
    // Закрытие не продажей на площадке плана — вышла из группы, свой возврат.
    let mut y = sold("D3", &dx, "G");
    y.relays = 1;
    let now = coin_group::route(&core, &mut y, vec![(b.clone(), d("7"))]).await;
    assert_eq!(now, vec![(b, d("7"))]);
    assert!(y.group.as_ref().unwrap().left);
}

#[tokio::test]
async fn group_return_once_lead_first() {
    let (core, s, _j) = start("group-first").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    let b = dx.sell_on.clone();
    riding(&core, &dx, "K", &["K", "K2"]).await;
    // Первая закрылась раньше — отложена, её уведомление ждёт общего возврата.
    let mut k = sold("K", &dx, "K");
    assert!(
        coin_group::route(&core, &mut k, vec![(b.clone(), d("100"))])
            .await
            .is_empty()
    );
    assert!(arb::core::deal_closed::hold(
        &mut k,
        Step::Closed,
        "продано"
    ));
    arb::core::ledger::save(&core, &k).await;
    // Последняя вложенная: возврат 0 «в сделке K», общий возврат — у K (п. 11).
    let mut x = sold("K2", &dx, "K");
    assert!(
        coin_group::route(&core, &mut x, vec![(b, d("95"))])
            .await
            .is_empty()
    );
    assert_eq!(x.group.as_ref().unwrap().shared_in.as_deref(), Some("K"));
    let k = wait_note(&core, "K", "последняя сделка группы продана").await;
    let sum = k.group.as_ref().unwrap().sum.clone().unwrap();
    assert_eq!(
        (sum.deals, sum.others, sum.usdt),
        (vec!["K2".to_string()], d("1"), d("95"))
    );
}

#[tokio::test]
async fn stuck_last_deal_does_not_hold_group_return() {
    let (core, s, _j) = start("group-stuck").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    let b = dx.sell_on.clone();
    riding(&core, &dx, "G", &["G", "D2"]).await;
    let mut g = sold("G", &dx, "G");
    assert!(
        coin_group::route(&core, &mut g, vec![(b, d("100"))])
            .await
            .is_empty()
    );
    arb::core::ledger::save(&core, &g).await;
    // Последняя идущая застряла — её не ждём: отложенная выручка уходит одним возвратом сразу (носитель — G).
    let stuck = bought("D2", &dx, Step::Stuck, ("40", "10"), Some("G"));
    arb::core::ledger::save(&core, &stuck).await;
    wait_note(&core, "G", "ушла с пути, её не ждём").await;
}

#[test]
fn won_deal_skips_direction_pause() {
    let dx = dir_of("bingx", "bsc_dex");
    let mut x = bought("D1", &dx, Step::Closed, ("40", "0"), Some("D1"));
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
