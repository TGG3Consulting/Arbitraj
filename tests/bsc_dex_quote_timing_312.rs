//! Файл: tests/bsc_dex_quote_timing_312.rs
//! Что делает: решение 312 — замер этапов котировки DEX (quote/quote_timing.rs) на записи маршрута CAKE
//! (tests/data/routes_2026-10-29, route_common: шесть настоящих пулов) в режиме dex.router_mode = graph, путь как в
//! Quoting::priced: quote_timing::start → cpu::run(timed(routing::run)). (1) log.quote_timing не задана — замера нет,
//! записей dex.quote_timing нет; (2) log.quote_timing = да и log.quote_timing_min_ms = 0 — на котировку запись есть, в
//! data все поля этапов и счётчиков, этапы классики и графа вызывались, сумма этапов верхнего уровня не больше работы,
//! части набора путей не больше самого набора, работа не больше всего. Конкретные миллисекунды не проверяются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{cpu, routing, quote_timing, quote_timing_stages}, tests/route_common.

mod route_common;

use std::time::Duration;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{Asset, Quote, Side, TokenTaxes};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::quote_timing::{self, Meta};
use arb::venues::bsc_dex::quote::quote_timing_stages::{COUNT_NAMES, STAGE_NAMES};
use arb::venues::bsc_dex::quote::{Quoting, cpu, routing};

use route_common::{CAKE, USDT};

const KIND: &str = "dex.quote_timing";

/// Окружение записи маршрута в режиме графа (как tests/bsc_dex_quote_cpu.rs).
async fn parts() -> Parts {
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("dex.router_mode", json!("graph")),
        ("dex.swap_mode", json!("direct")),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    p
}

/// Входные данные выбора маршрута для покупки CAKE (как их собирает quote/mod.rs).
fn input(q: &Quoting, qty: u32) -> routing::Input {
    let entries = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();
    let mids = q.mids.active(&q.ctx).into_iter().map(|m| (m, 18)).collect();
    routing::Input {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        side: Side::Buy,
        qty: Decimal::from(qty),
        coin: CAKE,
        usdt: USDT,
        taxes: TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        },
        coin_dec: 18,
        usdt_dec: 18,
        entries,
        mids,
        gas: U256::from(1_000_000_000u64),
        usdt_ticker: "USDT".into(),
        smart: false,
    }
}

/// Котировка тем же путём, что Quoting::priced: start (одно чтение настройки) → cpu::run(timed(routing::run)).
async fn quote(q: &Quoting, qty: u32) -> Quote {
    let timing = quote_timing::start(&q.ctx).map(|s| {
        let meta = Meta {
            ticker: "CAKE".into(),
            side: Side::Buy,
            qty: Decimal::from(qty),
        };
        (q.ctx.clone(), s, meta)
    });
    let work = quote_timing::timed(timing, routing::run(q.clone(), input(q, qty)));
    cpu::run(&q.ctx, Box::pin(work)).await.expect("котировка")
}

/// Записи dex.quote_timing в журнале.
async fn entries(q: &Quoting) -> Vec<Value> {
    let all = q.ctx.journal.list(1000, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == KIND)
        .filter_map(|e| e.data)
        .collect()
}

/// Число из data (поле обязано быть числом).
fn num(d: &Value, key: &str) -> f64 {
    d[key]
        .as_f64()
        .unwrap_or_else(|| panic!("поле {key} не число: {d}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_no_records_on_records_with_stages() {
    let p = parts().await;
    let q = p.quoting();
    // (1) Не задана — замера нет.
    assert!(quote_timing::start(&q.ctx).is_none());
    let off = quote(&q, 5_000).await;
    tokio::task::yield_now().await;
    assert!(entries(&q).await.is_empty(), "без настройки записей нет");
    // (2) Да и порог 0 — запись на каждую котировку.
    for (n, val) in [
        ("log.quote_timing", json!(true)),
        ("log.quote_timing_min_ms", json!(0)),
    ] {
        q.ctx
            .settings
            .set(n, Scope::Global, Some(val), "тест")
            .await
            .unwrap();
    }
    assert!(quote_timing::start(&q.ctx).is_some());
    let on = quote(&q, 5_000).await;
    // Замер не меняет котировку.
    assert_eq!(on.fillable_qty, off.fillable_qty);
    assert_eq!(on.avg_price, off.avg_price);
    assert_eq!(on.route, off.route);
    // Запись пишется отдельной задачей — ждём её появления (ограниченно).
    let mut got = Vec::new();
    for _ in 0..200 {
        got = entries(&q).await;
        if !got.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(got.len(), 1, "одна запись на котировку");
    let d = &got[0];
    assert_eq!(d["mode"], json!("graph"));
    assert_eq!(d["coin"], json!("CAKE"));
    assert_eq!(d["side"], json!("buy"));
    assert_eq!(d["ok"], json!(true));
    for (key, _) in STAGE_NAMES {
        num(d, &format!("{key}_ms"));
        num(d, &format!("{key}_n"));
    }
    for key in COUNT_NAMES {
        num(d, key);
    }
    // Этапы классики и графа вызывались.
    for key in ["classic", "paths", "legs", "merge"] {
        assert!(
            num(d, &format!("{key}_n")) >= 1.0,
            "этап {key} не замерен: {d}"
        );
    }
    assert!(num(d, "paths_in") >= 1.0);
    assert!(num(d, "hops") >= num(d, "paths_in"));
    assert!(num(d, "paths_ok") <= num(d, "paths_in"));
    // Допущенный путь есть — сплит, исполнимость и затраты считались.
    if num(d, "paths_ok") >= 1.0 {
        for key in ["split", "exec", "costs"] {
            assert!(num(d, &format!("{key}_n")) >= 1.0, "этап {key}: {d}");
        }
        assert!(num(d, "split_steps") >= 1.0);
    }
    // Сумма этапов верхнего уровня — не больше работы; работа — не больше всего; части путей — не больше путей.
    let eps = 1e-6;
    let top: f64 = STAGE_NAMES[..quote_timing::TOP]
        .iter()
        .map(|(k, _)| num(d, &format!("{k}_ms")))
        .sum();
    assert!(
        (top - num(d, "stages_ms")).abs() < 1e-3,
        "stages_ms — сумма этапов: {d}"
    );
    assert!(top <= num(d, "work_ms") + eps, "этапы больше работы: {d}");
    assert!(num(d, "work_ms") <= num(d, "total_ms") + eps, "{d}");
    assert!(num(d, "wait_ms") >= 0.0);
    let parts_of_paths: f64 = STAGE_NAMES[quote_timing::TOP..]
        .iter()
        .map(|(k, _)| num(d, &format!("{k}_ms")))
        .sum();
    assert!(
        parts_of_paths <= num(d, "paths_ms") + eps,
        "части путей больше путей: {d}"
    );
}
