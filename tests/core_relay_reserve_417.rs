//! Файл: tests/core_relay_reserve_417.rs
//! Что проверяет: решение 417 (Тигран 19.11.2026; живые сделки 19.11 THE −15.61, SNDKB −7.56, SPCXB, SINGULARRY
//! DEX → MEXC −7.25 — монета ехала 38–97 с, цена на цели за это время падала). Чисел этих сделок в tests/data нет —
//! проверка на записанных данных CAKE 2026-11-04 (tests/data/cex_samples_2026_11_04, записаны на сервере как есть):
//! - монета «здесь» — на Gate: средняя продажи здесь = лучший бид последнего кадра spot.order_book (gate_ws.jsonl,
//!   1.8909, на уровне 137.05 CAKE);
//! - цель — KuCoin: ряд σ продажи на цели — лучший бид снимков /spotMarket/level2Depth50 (kucoin_ws.jsonl, 11 кадров,
//!   время data.timestamp), средняя продажи на цели — бид последнего кадра (1.893, на уровне 330.1502 CAKE);
//! - количество 100 CAKE — в пределах первого уровня обоих стаканов; затрат перевода и ставки taker в записях нет —
//!   оценка цели без запаса = выручка цели (100 × 1.893), выручка здесь = 100 × 1.8909;
//! - T — границы времени перевода живых сделок 19.11 (38 и 97 с, profit.transit_manual_s площадки «здесь»).
//! Настоящее ядро с SQLite во временной папке (площадок в реестре нет, моков нет): σ, T, k и доля читаются
//! relay_reserve::params_at теми же функциями, что план входа.
//! (а) итог сделки при продаже здесь ≥ 0 — продажа здесь, цели не ищутся (here_first; в relay.rs проверка стоит раньше
//!     поиска целей и оценки перегона — по исходнику);
//! (б) минус здесь, цель лучше без запаса, но хуже с запасом (T = 97 с) — перегона нет, продажа здесь по
//!     stuck.relay_max_loss_pct (relay_here::decide);
//! (в) цель лучше и с запасом (T = 38 с) — перегон; в тексте хронологии — расчёт запаса.
//! Оговорка (ревью t201): ряд σ KuCoin в записи — 11 снимков и один шаг цены (1.894 → 1.893); границы (б)/(в) держатся на
//! этом одном шаге (запас при T = 97 — ≈0.30 USDT, при T = 38 — ≈0.19 USDT против перевеса цели 0.21 USDT) — тест
//! хрупкий к смене записи. Настоящий путь «Перегона» — tests/core_relay_live_417.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{relay_reserve, reserve, relay_here, deal, vol}, arb::settings, arb::storage.

use std::str::FromStr;

use alloy_primitives::address;
use arb::core::Core;
use arb::core::deal::Deal;
use arb::core::deps::Deps;
use arb::core::relay_here::{self, Here};
use arb::core::relay_reserve::{self, Est};
use arb::core::reserve;
use arb::core::types::{Asset, Direction, Side, VenueId};
use arb::core::vol::Volatility;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

const DIR: &str = "tests/data/cex_samples_2026_11_04";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// CAKE с адресом контракта из записанного ответа Gate (gate_wallet_currency_chains_CAKE.json, chain BSC).
fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(address!("0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82")),
    }
}

/// Кадры записи: строки JSON, поле msg — кадр площадки как пришёл.
fn frames(file: &str) -> Vec<Value> {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).unwrap();
    t.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|o| o.get("msg").and_then(Value::as_str).map(str::to_string))
        .filter_map(|m| serde_json::from_str::<Value>(&m).ok())
        .collect()
}

/// Лучший бид KuCoin по снимкам level2Depth50: (data.timestamp, цена).
fn kucoin_bids() -> Vec<(DateTime<Utc>, Decimal)> {
    frames("kucoin_ws.jsonl")
        .into_iter()
        .filter(|m| {
            m["topic"]
                .as_str()
                .is_some_and(|t| t.starts_with("/spotMarket/level2Depth50"))
        })
        .map(|m| {
            let t = Utc
                .timestamp_millis_opt(m["data"]["timestamp"].as_i64().unwrap())
                .unwrap();
            (t, d(m["data"]["bids"][0][0].as_str().unwrap()))
        })
        .collect()
}

/// Лучший бид Gate последнего кадра spot.order_book «update».
fn gate_last_bid() -> Decimal {
    let last = frames("gate_ws.jsonl")
        .into_iter()
        .rev()
        .find(|m| m["channel"] == "spot.order_book" && m["event"] == "update")
        .unwrap();
    d(last["result"]["bids"][0][0].as_str().unwrap())
}

/// Ядро на базе во временной папке.
async fn core(name: &str) -> (Core, Store) {
    let p = std::env::temp_dir().join(format!("arb_417_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    let db = Db::open(&p.join("arb.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(p.join("secrets.enc")),
        journal: journal.clone(),
    };
    let deps = Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: p.clone(),
    };
    (Core::start(deps).await, settings)
}

async fn set(s: &Store, name: &str, scope: Scope, val: Value) {
    s.set(name, scope, Some(val), "тест").await.unwrap();
}

/// Ядро с рядом σ продажи на KuCoin (напарник — Gate) из записи и настройками запаса; T — секунды перевода с Gate.
/// Возвращает: ядро, хранилище, «сейчас» (время последнего кадра KuCoin).
async fn stand(name: &str, transit_s: u64) -> (Core, Store, DateTime<Utc>) {
    let (c, s) = core(name).await;
    set(&s, "profit.sigma_window_min", Scope::Global, json!(60)).await;
    set(&s, "profit.price_move_k", Scope::Global, json!("1")).await;
    let gate = Scope::Venue("gate".into());
    set(&s, "profit.transit_mode", gate.clone(), json!("manual")).await;
    set(&s, "profit.transit_manual_s", gate, json!(transit_s)).await;
    let ku = Scope::Venue("kucoin".into());
    set(&s, "profit.closure_share", ku, json!("0.5")).await;
    let bids = kucoin_bids();
    {
        let mut vol = c.inner.vol.lock().unwrap();
        for (t, p) in &bids {
            vol.sample(
                &v("kucoin"),
                &cake(),
                Side::Sell,
                &v("gate"),
                *p,
                *t,
                Some(60),
            );
        }
    }
    let now = bids.last().unwrap().0;
    (c, s, now)
}

/// Сделка CAKE, монета на Gate (100 CAKE), вложено `invested`.
fn deal(invested: &str) -> Deal {
    let dir = Direction {
        asset: cake(),
        buy_on: v("bsc_dex"),
        sell_on: v("gate"),
    };
    let mut x = Deal::new(
        "20261119000000417CAKE".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.at = v("gate");
    x.qty = d("100");
    x.invested = d(invested);
    x
}

/// Оценка цели KuCoin на стенде с временем перевода `transit_s`.
async fn est(name: &str, transit_s: u64) -> Est {
    let (c, s, now) = stand(name, transit_s).await;
    let p = relay_reserve::params_at(&c, &v("gate"), &v("kucoin"), &cake(), now).unwrap();
    // Параметры — те же функции, что у плана входа: σ — как reserve.rs sigma_now по тому же ряду, keep — keep_of.
    let dir = Direction {
        asset: cake(),
        buy_on: v("gate"),
        sell_on: v("kucoin"),
    };
    let mut own = Volatility::default();
    for (t, px) in kucoin_bids() {
        own.sample(
            &v("kucoin"),
            &cake(),
            Side::Sell,
            &v("gate"),
            px,
            t,
            Some(60),
        );
    }
    assert_eq!(
        p.sigma,
        reserve::sigma_now(&mut own, &s, &dir, now).unwrap()
    );
    assert_eq!(p.transit_s, Decimal::from(transit_s));
    assert_eq!(p.k, Decimal::ONE);
    assert_eq!(p.share, Some(d("0.5")));
    let t = Decimal::from(transit_s);
    assert_eq!(Some(p.keep), reserve::keep_of(p.k, p.sigma, t));
    let (qty, tgt, here) = (d("100"), d("1.893"), gate_last_bid());
    assert_eq!(kucoin_bids().last().unwrap().1, tgt);
    assert_eq!(here, d("1.8909"));
    let proceeds = qty * tgt;
    let e = relay_reserve::estimate(proceeds, proceeds, tgt, Some(here), p.clone());
    // Запас — ровно reserve.rs amount плана входа (цена «покупки» плана — средняя продажи здесь).
    assert_eq!(
        e.reserve,
        reserve::amount(p.keep, proceeds, p.share, here, tgt)
    );
    e
}

/// (а) Итог сделки при продаже здесь ≥ 0 — продажа здесь, перегон не рассматривается.
#[tokio::test]
async fn profitable_here_sells_here_without_search() {
    let here = d("100") * gate_last_bid();
    // Вложено 189: итог при продаже здесь 189.09 − 189 = +0.09.
    assert!(relay_reserve::here_first(&deal("189"), Some(here)));
    // Ровно 0 — тоже продажа здесь (решение 47: итог не ниже нуля).
    assert!(relay_reserve::here_first(&deal("189.09"), Some(here)));
    // Минус или котировки нет — дальше оценка целей.
    assert!(!relay_reserve::here_first(&deal("190"), Some(here)));
    assert!(!relay_reserve::here_first(&deal("189"), None));
    // По исходнику relay.rs: проверка — раньше поиска целей и оценки перегона.
    let src = std::fs::read_to_string("src/core/relay.rs").unwrap();
    let first = src.find("relay_reserve::here_first(").unwrap();
    for what in [
        "targets_why(core, &deal)",
        "relay_value::value_parts(",
        "transfers::send_trade(",
    ] {
        let at = src.find(what).unwrap_or_else(|| panic!("нет {what}"));
        assert!(first < at, "{what} раньше правила 1");
    }
}

/// (б) Минус здесь; цель лучше без запаса, но хуже с запасом (T = 97 с) — продажа здесь по прежним правилам.
#[tokio::test]
async fn better_raw_worse_reserved_sells_here() {
    let e = est("b", 97).await;
    let here = d("100") * gate_last_bid();
    let x = deal("190");
    assert!(!relay_reserve::here_first(&x, Some(here)));
    // Без запаса цель лучше (189.30 > 189.09) — прежнее строгое сравнение увезло бы монету.
    assert!(e.raw > here);
    // Доля закрытия 0.5 · (1.893 − 1.8909) · 100 меньше базовой части — запас = базовая часть.
    assert!(e.closure.unwrap() < e.base);
    assert_eq!(e.reserve, e.base);
    assert!(e.net() < here, "{e:?}");
    assert!(!relay_reserve::better(Some(here), &e));
    // Продажа здесь — по stuck.relay_max_loss_pct (решение 220): убыток 0.91 / 190 ≈ 0.48 % ≤ 1 % — по рынку.
    match relay_here::decide(&x, "перегон не выгоднее", Ok(Decimal::ONE), Ok(here))
    {
        Here::Market(t) => assert!(t.contains("stuck.relay_max_loss_pct"), "{t}"),
        h => panic!("{h:?}"),
    }
}

/// (в) Цель лучше и с запасом (T = 38 с) — перегон; в строке хронологии — расчёт запаса.
#[tokio::test]
async fn better_reserved_relays() {
    let e = est("c", 38).await;
    let here = d("100") * gate_last_bid();
    assert!(!relay_reserve::here_first(&deal("190"), Some(here)));
    assert!(e.reserve > Decimal::ZERO);
    assert!(e.net() > here, "{e:?}");
    assert!(relay_reserve::better(Some(here), &e));
    // Котировки здесь нет — сравнивать не с чем (как раньше): перегон.
    assert!(relay_reserve::better(None, &e));
    let t = relay_reserve::text(&e);
    for part in [
        "ожидается",
        "без запаса",
        "запас",
        "k·σ·√T=",
        "T=38 с",
        "доля закрытия=0.5 →",
    ] {
        assert!(t.contains(part), "{part}: {t}");
    }
}

/// Нет данных для запаса — цель не оценена (как план входа): ряда σ нет; не задана k.
#[tokio::test]
async fn no_reserve_data_is_error() {
    let (c, s, now) = stand("e", 97).await;
    // Ряда продажи на MEXC с напарником Gate нет.
    assert!(relay_reserve::params_at(&c, &v("gate"), &v("mexc"), &cake(), now).is_err());
    s.set("profit.price_move_k", Scope::Global, None, "тест")
        .await
        .unwrap();
    let r = relay_reserve::params_at(&c, &v("gate"), &v("kucoin"), &cake(), now);
    assert!(r.unwrap_err().contains("profit.price_move_k"));
}
