//! Файл: tests/bsc_dex_quote_timing_312.rs
//! Что делает: решение 312 — замер этапов котировки DEX (quote/quote_timing.rs) на записи маршрута CAKE
//! (tests/data/routes_2026-10-29, route_common: шесть настоящих пулов) в режиме dex.router_mode = graph, путь как в
//! Quoting::priced: Outer::begin → cpu::run(timed(Outer::run, routing::run)) → Outer::back → Outer::finish (без частей до
//! и после работы — decimals и цена газа здесь не запрашиваются). (1) log.quote_timing не задана — замера нет, записей
//! dex.quote_timing нет; (2) log.quote_timing = да и log.quote_timing_min_ms = 0 — на котировку запись есть, в data все
//! поля этапов, частей и счётчиков, этапы классики и графа (и подготовка глубины) вызывались, сумма этапов верхнего
//! уровня не больше работы, rest_ms = work_ms − stages_ms, total_ms = pre_ms + run_ms + post_ms, run_ms = wait_ms +
//! work_ms + back_ms, части набора путей не больше самого набора. (3) Порог больше нуля (час, задан в тесте) отсекает
//! запись; после котировки с замером котировка без замера записи не даёт и признак потока счёта снят (сброс
//! состояния). (4) Котировка брошена в cpu::run до старта работы (Outer::run отдан, работа не начата, Outer брошен) —
//! запись Drop: dropped = да, unstarted = да, полей работы нет; связь закрыта — старт работы после этого без замера
//! (quote_timing_outer::start = нет), работа той же связи второй записи не даёт. Конкретные миллисекунды не проверяются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{cpu, routing, quote_timing, quote_timing_outer, quote_timing_stages},
//! tests/route_common.

mod route_common;

use std::time::Duration;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{Asset, Quote, Side, TokenTaxes};
use arb::gateway::errors::GatewayError;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::quote_timing;
use arb::venues::bsc_dex::quote::quote_timing_outer::{self, Outcome, Outer};
use arb::venues::bsc_dex::quote::quote_timing_stages::{COUNT_NAMES, PART_NAMES, STAGE_NAMES};
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
            chain: None,
            ticker: "CAKE".into(),
            contract: Some(CAKE),
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

/// Котировка тем же путём, что Quoting::priced: Outer::begin (одно чтение настройки) → cpu::run(timed(routing::run)) →
/// back → finish (запись после хвоста).
async fn quote(q: &Quoting, qty: u32) -> Quote {
    let mut tm = Outer::begin(&q.ctx, "CAKE", Side::Buy, Decimal::from(qty));
    let work = quote_timing::timed(tm.run(), routing::run(q.clone(), input(q, qty)));
    let out = cpu::run(&q.ctx, Box::pin(work)).await;
    tm.back();
    tm.finish(|| Outcome::of(&out));
    out.expect("котировка")
}

/// Задаёт log.quote_timing и log.quote_timing_min_ms (вся система).
async fn set(q: &Quoting, on: bool, min_ms: u64) {
    for (n, val) in [
        ("log.quote_timing", json!(on)),
        ("log.quote_timing_min_ms", json!(min_ms)),
    ] {
        q.ctx
            .settings
            .set(n, Scope::Global, Some(val), "тест")
            .await
            .unwrap();
    }
}

/// Ждёт (ограниченно), пока записей dex.quote_timing станет не меньше `n`; возвращает записи.
async fn wait_n(q: &Quoting, n: usize) -> Vec<Value> {
    let mut got = Vec::new();
    for _ in 0..200 {
        got = entries(q).await;
        if got.len() >= n {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    got
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
    assert!(!Outer::begin(&q.ctx, "CAKE", Side::Buy, Decimal::ONE).is_on());
    let off = quote(&q, 5_000).await;
    tokio::task::yield_now().await;
    assert!(entries(&q).await.is_empty(), "без настройки записей нет");
    // (2) Да и порог 0 — запись на каждую котировку.
    set(&q, true, 0).await;
    let on = quote(&q, 5_000).await;
    // Замер не меняет котировку.
    assert_eq!(on.fillable_qty, off.fillable_qty);
    assert_eq!(on.avg_price, off.avg_price);
    assert_eq!(on.route, off.route);
    // Запись пишется отдельной задачей — ждём её появления (ограниченно).
    let got = wait_n(&q, 1).await;
    assert_eq!(got.len(), 1, "одна запись на котировку");
    let d = &got[0];
    assert_eq!(d["mode"], json!("graph"));
    assert_eq!(d["coin"], json!("CAKE"));
    assert_eq!(d["side"], json!("buy"));
    assert_eq!(d["ok"], json!(true));
    assert_eq!(d["fast"], json!(false));
    assert_eq!(d["dropped"], json!(false));
    for (key, _) in STAGE_NAMES {
        num(d, &format!("{key}_ms"));
        num(d, &format!("{key}_n"));
    }
    for (key, _) in PART_NAMES {
        num(d, &format!("{key}_ms"));
    }
    for key in COUNT_NAMES {
        num(d, key);
    }
    // Этапы классики и графа вызывались.
    for key in ["classic", "paths", "prep", "legs", "merge"] {
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
    // Остаток вне этапов — явно; всё = до работы + cpu::run + после; cpu::run = ожидание + работа + возврат.
    let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
    assert!(
        close(num(d, "rest_ms"), num(d, "work_ms") - num(d, "stages_ms")),
        "rest_ms: {d}"
    );
    let parts_sum = num(d, "pre_ms") + num(d, "run_ms") + num(d, "post_ms");
    assert!(close(parts_sum, num(d, "total_ms")), "total_ms: {d}");
    let run_sum = num(d, "wait_ms") + num(d, "work_ms") + num(d, "back_ms");
    assert!(close(run_sum, num(d, "run_ms")), "run_ms: {d}");
    let parts_of_paths: f64 = STAGE_NAMES[quote_timing::TOP..quote_timing::PATHS_END]
        .iter()
        .map(|(k, _)| num(d, &format!("{k}_ms")))
        .sum();
    assert!(
        parts_of_paths <= num(d, "paths_ms") + eps,
        "части путей больше путей: {d}"
    );
    // Решение 336: части классики — своя группа (PATHS_END..): один пул, промежуточная и выбор идут по очереди; допуск
    // пулов и затраты — внутри них и друг с другом не пересекаются; каждая — не больше классики.
    let classic = num(d, "classic_ms");
    for (k, _) in &STAGE_NAMES[quote_timing::PATHS_END..] {
        assert!(
            num(d, &format!("{k}_ms")) <= classic + eps,
            "часть классики {k} больше классики: {d}"
        );
    }
    let seq = num(d, "classic_one_ms") + num(d, "classic_via_ms") + num(d, "classic_pick_ms");
    assert!(
        seq <= classic + eps,
        "один пул + промежуточная + выбор больше классики: {d}"
    );
    let inner = num(d, "classic_admit_ms") + num(d, "classic_costs_ms");
    assert!(
        inner <= classic + eps,
        "допуск + затраты больше классики: {d}"
    );
    assert!(num(d, "admit_hits") <= num(d, "admit_calls"), "{d}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn threshold_cuts_and_state_resets() {
    let p = parts().await;
    let q = p.quoting();
    // (а) Порог заведомо больше котировки (час, задан здесь) — записи нет.
    set(&q, true, 3_600_000).await;
    quote(&q, 5_000).await;
    // (б) Котировка с замером (порог 0) — одна запись; затем замер выключен — котировка записи не даёт.
    set(&q, true, 0).await;
    quote(&q, 5_000).await;
    assert_eq!(wait_n(&q, 1).await.len(), 1, "котировка с замером");
    set(&q, false, 0).await;
    quote(&q, 5_000).await;
    // Признак замера потока счёта снят после работы (thread_local; блокирующий поток переиспользуется).
    let flag = cpu::run(
        &q.ctx,
        Box::pin(async { Ok::<bool, GatewayError>(quote_timing::active()) }),
    );
    let flag = flag.await.unwrap();
    assert!(!flag, "признак замера остался в потоке счёта");
    // Контроль: снова с замером — записей ровно две (ни (а), ни котировка без замера записи не дали). Записи пишутся
    // отдельными задачами — после появления второй ждём ещё, чтобы запоздавшая лишняя успела бы появиться.
    set(&q, true, 0).await;
    quote(&q, 5_000).await;
    wait_n(&q, 2).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(entries(&q).await.len(), 2, "лишняя запись");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_before_work_start_writes_once() {
    let p = parts().await;
    let q = p.quoting();
    set(&q, true, 0).await;
    // Вход в cpu::run отмечен, работа в потоке счёта не начата — priced брошен (как в ожидании места dex.quote_cpu_max).
    let mut tm = Outer::begin(&q.ctx, "CAKE", Side::Buy, Decimal::from(5_000u32));
    let run = tm.run().expect("замер включён");
    drop(tm);
    let got = wait_n(&q, 1).await;
    assert_eq!(got.len(), 1, "запись брошенной до старта работы");
    let d = &got[0];
    assert_eq!(d["dropped"], json!(true));
    assert_eq!(d["unstarted"], json!(true));
    assert_eq!(d["work_ms"], Value::Null);
    assert_eq!(d["stages_ms"], Value::Null);
    // Связь закрыта: старт работы — без замера; работа с этой связью записи не даёт.
    assert!(!quote_timing_outer::start(&run));
    let work = quote_timing::timed(Some(run), routing::run(q.clone(), input(&q, 5_000)));
    cpu::run(&q.ctx, Box::pin(work)).await.expect("котировка");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(entries(&q).await.len(), 1, "лишняя запись");
}
