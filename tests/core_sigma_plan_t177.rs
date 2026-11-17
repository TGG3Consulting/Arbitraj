//! Файл: tests/core_sigma_plan_t177.rs
//! Что проверяет: решение 391 (t177) (core/sigma_plan.rs entry) на настоящем ядре с SQLite (настройки в хранилище) и
//! записанном ряде — лучший бид Gate CAKE_USDT из записанного потока spot.order_book с time_ms
//! (tests/data/cex_samples_2026_11_04/gate_ws.jsonl), направление bsc_dex → gate (как tests/core_price_reserve.rs):
//! 1) ряд с изменениями цены и минимум выполнен — σ та же, что reserve.rs sigma_now (с границей и без);
//! 2) неподвижный хвост записи (бид не менялся, 0 изменений) при profit.sigma_min_changes = 1:
//!    - граница profit.sigma_floor_per_s [gate] не задана — отказ, тот же текст, что у sigma_now;
//!    - граница задана — σ плана = граница (σ ряда = 0);
//!    - граница 0 или граница другой площадки — отказ, как раньше;
//! 3) новое направление (один замер, шагов нет) — отказ и при заданной границе.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{sigma_plan, reserve, vol}. Моков нет.

use std::str::FromStr;

use alloy_primitives::address;
use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::reserve;
use arb::core::sigma_plan;
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

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(address!("0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82")),
    }
}

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

fn gd() -> Direction {
    Direction {
        asset: cake(),
        buy_on: v("bsc_dex"),
        sell_on: v("gate"),
    }
}

/// Настоящее ядро без площадок (как tests/core_price_reserve.rs).
async fn core(name: &str) -> (Core, Store) {
    let p = std::env::temp_dir().join(format!("arb_sigma_plan_{name}_{}", std::process::id()));
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

async fn set(s: &Store, name: &str, scope: Scope, v: Value) {
    s.set(name, scope, Some(v), "тест").await.unwrap();
}

/// Записанный лучший бид Gate CAKE_USDT: (time_ms кадра, цена) по кадрам spot.order_book «update».
fn gate_bids() -> Vec<(DateTime<Utc>, Decimal)> {
    let raw = std::fs::read_to_string("tests/data/cex_samples_2026_11_04/gate_ws.jsonl").unwrap();
    raw.lines()
        .filter_map(|l| {
            let o: Value = serde_json::from_str(l).unwrap();
            let m: Value = serde_json::from_str(o["msg"].as_str()?).unwrap();
            (m["channel"] == "spot.order_book" && m["event"] == "update").then(|| {
                let t = Utc
                    .timestamp_millis_opt(m["time_ms"].as_i64().unwrap())
                    .unwrap();
                (t, d(m["result"]["bids"][0][0].as_str().unwrap()))
            })
        })
        .collect()
}

/// Ряд σ направления bsc_dex → gate из кадров; окно 60 мин.
fn series(frames: &[(DateTime<Utc>, Decimal)]) -> Volatility {
    let mut vol = Volatility::default();
    for (t, p) in frames {
        vol.sample(
            &v("gate"),
            &cake(),
            Side::Sell,
            &v("bsc_dex"),
            *p,
            *t,
            Some(60),
        );
    }
    vol
}

#[tokio::test]
async fn floor_instead_of_unmeasured() {
    let (_core, s) = core("floor").await;
    set(&s, "profit.sigma_window_min", Scope::Global, json!(60)).await;
    let frames = gate_bids();
    let now = frames.last().unwrap().0;
    let gate = Scope::Venue("gate".into());
    // 1) Ряд с изменениями, минимум 1 выполнен — как sigma_now (без границы и с границей).
    set(&s, "profit.sigma_min_changes", Scope::Global, json!(1)).await;
    let mut vol = series(&frames);
    let was = reserve::sigma_now(&mut vol, &s, &gd(), now).unwrap();
    assert!(was > Decimal::ZERO);
    assert_eq!(sigma_plan::entry(&mut vol, &s, &gd(), now).unwrap(), was);
    set(
        &s,
        "profit.sigma_floor_per_s",
        gate.clone(),
        json!((was / d("2")).to_string()),
    )
    .await;
    assert_eq!(sigma_plan::entry(&mut vol, &s, &gd(), now).unwrap(), was);
    s.set("profit.sigma_floor_per_s", gate.clone(), None, "тест")
        .await
        .unwrap();
    // 2) Неподвижный хвост записи: 0 изменений < 1.
    let last = frames.last().unwrap().1;
    let from = frames.iter().rposition(|f| f.1 != last).unwrap() + 1;
    let quiet = &frames[from..];
    assert!(quiet.len() >= 2, "в записи есть неподвижный хвост");
    let mut qv = series(quiet);
    let old = reserve::sigma_now(&mut qv, &s, &gd(), now).unwrap_err();
    let e = sigma_plan::entry(&mut qv, &s, &gd(), now).unwrap_err();
    assert_eq!(e, old);
    assert!(e.contains("цены 0, нужно не меньше 1"), "{e}");
    // Граница другой площадки — не действует.
    set(
        &s,
        "profit.sigma_floor_per_s",
        Scope::Venue("bingx".into()),
        json!("0.0002"),
    )
    .await;
    assert!(sigma_plan::entry(&mut qv, &s, &gd(), now).is_err());
    // Граница 0 — отказ, как раньше (иначе запас 0 — то, от чего защищает минимум, решение 240).
    set(&s, "profit.sigma_floor_per_s", gate.clone(), json!("0")).await;
    assert!(sigma_plan::entry(&mut qv, &s, &gd(), now).is_err());
    // Граница задана — σ плана = граница.
    set(
        &s,
        "profit.sigma_floor_per_s",
        gate.clone(),
        json!("0.000155"),
    )
    .await;
    assert_eq!(
        sigma_plan::entry(&mut qv, &s, &gd(), now).unwrap(),
        d("0.000155")
    );
    // 3) Новое направление: один замер, шагов нет — отказ и при границе.
    let mut one = series(&quiet[..1]);
    assert!(sigma_plan::entry(&mut one, &s, &gd(), now).is_err());
}
