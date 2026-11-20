//! Файл: tests/core_pool_own_t238.rs
//! Что делает: решение 463 (t238) — учёт своих идущих сделок через один пул на записанных сделках NUM живого бота
//! 20.11.2026 (tests/data/num_pool_2026-11-20/deals.json: Gate → DEX и три KuCoin → DEX, пул PCS V3 0xf89068…371d),
//! на настоящем ядре и SQLite во временной папке (площадок в реестре нет, моков нет):
//! - сдвиг от идущих сделок ДРУГОЙ биржи покупки: продажа новой KuCoin-сделки — после монет сделки Gate (как в живом
//!   плане), вложенная KuCoin входит в группу первой KuCoin (как в записи: group 20261120180725680NUM); продажа на
//!   бирже (bsc_dex → kucoin) не считает монеты, которые продадут в DEX;
//! - уже исполненная продажа (учёт уменьшен в момент итога, credit_lock::selling) в сдвиг больше не входит, след с её
//!   блоком ставится сразу: котировка по блоку ниже её блока — отказ, по её блоку — принята; при двух продажах блок
//!   следа — самый высокий;
//! - продажа в полёте (ход очереди продажи держит sell_turn), исполнившаяся за время котировки (счётчик итогов) и с
//!   неизвестным исходом — котировки нет (ride_settled.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (coin_ride, ride_quote, ride_trail, ride_settled, credit_lock, sell_turn, ledger),
//! arb::settings, arb::storage, arb::notify, tests/data/num_pool_2026-11-20.

use alloy_primitives::{Address, B256};
use arb::core::Core;
use arb::core::coin_key::CoinKey;
use arb::core::coin_ride::Seat;
use arb::core::deal::{Deal, Step};
use arb::core::deal_group::DealGroup;
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::types::{
    Asset, ChainTx, DexRoute, Direction, PoolKind, PoolRef, Quote, RouteHop, RouteLeg, RouteMode,
    SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome, VenueId,
};
use arb::core::{coin_ride, credit_lock, ledger, ride_quote, ride_settled, ride_trail, sell_turn};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Запись сделок NUM (data).
fn rec() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/num_pool_2026-11-20/deals.json"
    );
    let text = std::fs::read_to_string(p).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Decimal из строкового поля записи.
fn dv(v: &Value, k: &str) -> Decimal {
    v[k].as_str().unwrap().parse().unwrap()
}

/// Время из строкового поля записи.
fn tv(v: &Value, k: &str) -> DateTime<Utc> {
    v[k].as_str().unwrap().parse().unwrap()
}

/// Сделка записи по ID.
fn one<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["deals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id)
        .unwrap()
}

/// Строковое поле записи.
fn sv<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap()
}

/// Монета записи.
fn asset(r: &Value) -> Asset {
    let a = &r["asset"];
    Asset {
        ticker: sv(a, "ticker").into(),
        bsc_contract: Some(sv(a, "bsc_contract").parse().unwrap()),
    }
}

/// Направление монеты записи A → B.
fn dir(r: &Value, a: &str, b: &str) -> Direction {
    Direction {
        asset: asset(r),
        buy_on: VenueId(a.into()),
        sell_on: VenueId(b.into()),
    }
}

/// Ядро на базе во временной папке; gate, kucoin — биржи, bsc_dex — DEX. Включена докупка (exec.top_up = да, как
/// на живом боте 20.11). Возвращает ядро.
async fn start(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-t238-{name}-{}", std::process::id()));
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
        data_dir: d.clone(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("gate".into()), VenueKind::Cex),
        (VenueId("kucoin".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    settings
        .set(coin_ride::TOP_UP, Scope::Global, Some(json!(true)), "тест")
        .await
        .unwrap();
    core
}

/// Сделка записи на шаге «перевод»: купленное количество и вложенное — из записи, группа — из записи.
fn in_transit(r: &Value, id: &str) -> Deal {
    let x = one(r, id);
    let d = dir(r, sv(x, "buy_on"), sv(x, "sell_on"));
    let mut deal = Deal::new(id.into(), d.clone(), json!({}), Decimal::ONE);
    deal.group = Some(Box::new(DealGroup::new(sv(x, "group"))));
    deal.step = Step::Transfer;
    deal.qty = dv(x, "buy_filled");
    deal.invested = dv(x, "buy_filled") * dv(x, "buy_avg");
    deal
}

/// Маршрут продажи через пул записи по блоку.
fn route(r: &Value, block: u64) -> DexRoute {
    DexRoute {
        mode: RouteMode::Direct,
        legs: vec![RouteLeg {
            share: Decimal::ONE,
            hops: vec![RouteHop {
                family: sv(r, "pool_family").into(),
                kind: PoolKind::V3,
                pool: PoolRef::Contract(sv(r, "pool").parse::<Address>().unwrap()),
            }],
            expected_out: Decimal::ZERO,
        }],
        block,
        gas_units: 0,
        executable: true,
        exec_note: None,
    }
}

/// Продажа сделки записи (итог ордера): количество, средняя, конец, блок, маршрут запроса — из записи.
fn sale(r: &Value, id: &str) -> TradeResult {
    let s = &one(r, id)["sell"];
    TradeResult {
        request: TradeRequest {
            venue: VenueId("bsc_dex".into()),
            asset: asset(r),
            side: Side::Sell,
            qty: dv(s, "qty"),
            worst_price: dv(s, "worst"),
            deal_id: id.into(),
            route: Some(route(r, s["route_block"].as_u64().unwrap())),
            exit_position: true,
            remeasure: None,
        },
        status: TradeStatus::Filled,
        filled_qty: dv(s, "qty"),
        avg_price: dv(s, "avg"),
        fees: vec![],
        venue_ref: id.into(),
        chain: Some(ChainTx {
            hash: B256::ZERO,
            nonce: 0,
            channels: vec![],
            method: SendMethod::Single,
            outcome: TxOutcome::Included,
            block: s["block"].as_u64(),
            gas_used: None,
            gas_bnb: None,
            tip_bnb: None,
            cancel_hash: None,
            cancel_gas_bnb: None,
            approve_gas_bnb: None,
            tip_hash: None,
            tip_gas_bnb: None,
        }),
        finished_at: tv(s, "finished_at"),
        executor: None,
    }
}

/// Котировка продажи на bsc_dex через пул записи по блоку (для проверки следа — важны маршрут и блок).
fn quote_at(r: &Value, block: u64) -> Quote {
    Quote {
        venue: VenueId("bsc_dex".into()),
        asset: asset(r),
        side: Side::Sell,
        qty: Decimal::ONE,
        fillable_qty: Decimal::ONE,
        avg_price: Decimal::ONE,
        costs: vec![],
        route: Some(route(r, block)),
        signal_route: None,
        pick: None,
        at: Utc::now(),
    }
}

/// Вход сделки как у исполнителя (admit → учёт → board). Возвращает место.
async fn ride(core: &Core, deal: &Deal) -> Seat {
    let seat = coin_ride::admit(core, &deal.direction).unwrap();
    ledger::save(core, deal).await;
    let g = deal.group.as_ref().map(|g| g.id.clone());
    coin_ride::board(core, &deal.direction, &deal.id, g);
    seat
}

/// Монеты сделки зачислены на bsc_dex (количество зачисления — из записи), шаг «продажа».
async fn credited(core: &Core, r: &Value, deal: &mut Deal) {
    deal.qty = dv(one(r, &deal.id), "credited");
    deal.at = VenueId("bsc_dex".into());
    deal.step = Step::Selling;
    ledger::save(core, deal).await;
}

const GATE: &str = "20261120180718098NUM";
const K1: &str = "20261120180725680NUM";
const K2: &str = "20261120180733956NUM";
const K3: &str = "20261120180737357NUM";

#[tokio::test]
async fn other_exchange_rides_shift_the_pool() {
    let r = rec();
    let core = start("shift").await;
    let gate = in_transit(&r, GATE);
    assert_eq!(ride(&core, &gate).await, Seat::Lead);
    let ku = dir(&r, "kucoin", "bsc_dex");
    // Новая KuCoin-сделка: впереди — монеты сделки Gate (другая биржа покупки, та же площадка продажи).
    assert_eq!(ride_quote::sell_ahead(&core, &ku), gate.qty);
    assert_eq!(ride(&core, &in_transit(&r, K1)).await, Seat::Lead);
    // Вложенная KuCoin — в группу первой KuCoin, как в записи; впереди неё — Gate и первая KuCoin.
    let k2 = in_transit(&r, K2);
    let before = ride_quote::sell_ahead(&core, &ku);
    assert_eq!(before, gate.qty + dv(one(&r, K1), "buy_filled"));
    assert_eq!(ride(&core, &k2).await, Seat::Join(K1.into()));
    assert_eq!(ride_quote::sell_ahead(&core, &ku), before + k2.qty);
    // Продажа на бирже (DEX → KuCoin) пул не трогает: монеты, которые продадут в DEX, не впереди.
    assert_eq!(
        ride_quote::sell_ahead(&core, &dir(&r, "bsc_dex", "kucoin")),
        Decimal::ZERO
    );
}

#[tokio::test]
async fn executed_sale_leaves_projection_and_sets_block() {
    let r = rec();
    let core = start("sold").await;
    let mut all = Vec::new();
    for id in [GATE, K1, K2, K3] {
        let x = in_transit(&r, id);
        ride(&core, &x).await;
        all.push(x);
    }
    let ku = dir(&r, "kucoin", "bsc_dex");
    for x in all.iter_mut().skip(1) {
        credited(&core, &r, x).await;
    }
    let ahead = ride_quote::sell_ahead(&core, &ku);
    let credited_k: Decimal = [K1, K2, K3]
        .iter()
        .map(|i| dv(one(&r, i), "credited"))
        .sum();
    // В пути — купленное, зачисленные — зачисленное.
    assert_eq!(ahead, all[0].qty + credited_k);
    // Продажа K1 исполнена (блок 134740502): учёт уменьшен в момент итога, сдвиг — только остальные.
    let s1 = sale(&r, K1);
    let f = async { Ok::<_, OpError>(s1.clone()) };
    assert!(credit_lock::selling(&core, &all[1], f).await.is_ok());
    assert_eq!(ride_quote::sell_ahead(&core, &ku), ahead - s1.filled_qty);
    let key = CoinKey::of(&ku.asset);
    let v = VenueId("bsc_dex".into());
    // Котировка по блоку маршрута её запроса (134740500) — пул ещё без её продажи: отказ, монеты не потеряны.
    let (q500, q502) = (quote_at(&r, 134740500), quote_at(&r, 134740502));
    let early = ride_trail::check(&core, &key, &v, &q500);
    assert!(early.as_ref().is_err_and(|e| e.contains(K1)), "{early:?}");
    assert!(ride_trail::check(&core, &key, &v, &q502).is_ok());
    // K2 (блок 134740509) и K3 (блок 134740505, кончилась раньше по времени): блок следа — самый высокий.
    for (i, id) in [(2usize, K2), (3, K3)] {
        let f = async { Ok::<_, OpError>(sale(&r, id)) };
        assert!(credit_lock::selling(&core, &all[i], f).await.is_ok());
    }
    assert_eq!(ride_quote::sell_ahead(&core, &ku), all[0].qty);
    let (q505, q509) = (quote_at(&r, 134740505), quote_at(&r, 134740509));
    assert!(ride_trail::check(&core, &key, &v, &q505).is_err());
    assert!(ride_trail::check(&core, &key, &v, &q509).is_ok());
}

#[tokio::test]
async fn sale_in_flight_or_unknown_has_no_quote() {
    let r = rec();
    let core = start("flight").await;
    let mut gate = in_transit(&r, GATE);
    ride(&core, &gate).await;
    credited(&core, &r, &mut gate).await;
    let ku = dir(&r, "kucoin", "bsc_dex");
    let seq = ride_settled::before(&core, &ku).unwrap();
    assert!(ride_settled::after(&core, &ku, seq).is_ok());
    // Продажа Gate держит ход (котировка → итог ордера): попала ли в пул, не видно — котировки нет.
    let coin = gate.direction.asset.clone();
    let turn = sell_turn::try_take(&gate.at, &coin).unwrap();
    let busy = ride_settled::before(&core, &ku).unwrap_err();
    assert!(busy.contains("идёт наша продажа NUM"), "{busy}");
    assert!(busy.contains(GATE), "{busy}");
    assert!(ride_settled::after(&core, &ku, seq).is_err());
    // Продажа исполнилась, пока шла котировка: счётчик итогов сменился — отказ; следующая котировка — по новому.
    let s = sale(&r, GATE);
    turn.done(&core, &gate, s.request.route.as_ref(), &Ok(s.clone()));
    let done = ride_settled::after(&core, &ku, seq).unwrap_err();
    assert!(done.contains("исполнилась, пока шла котировка"), "{done}");
    let seq2 = ride_settled::before(&core, &ku).unwrap();
    assert!(ride_settled::after(&core, &ku, seq2).is_ok());
    // Исход нашей продажи неизвестен — котировки нет.
    gate.sale_unsure = true;
    ledger::save(&core, &gate).await;
    let unsure = ride_settled::before(&core, &ku).unwrap_err();
    assert!(
        unsure.contains("исход нашей продажи NUM неизвестен"),
        "{unsure}"
    );
}
