//! Файл: tests/bsc_dex_quote_reuse_336.rs
//! Что делает: решение 336 (ревью t157 Н6) на записи маршрута CAKE (tests/data/routes_2026-10-29, route_common: шесть
//! настоящих пулов, промежуточная WBNB): котировка с коробом подбора и повторным использованием (gateway/calc_share.rs,
//! reuse = да — допуск пулов классики quote/usable_reuse.rs, пути и допуск хопов графа quote/graph_reuse.rs, обмен пула
//! монеты пути через промежуточную один раз на пул — quote/route_first.rs, route_calc.rs eval_from) на одном и том же
//! состоянии реестра даёт ту же котировку, что без короба: побитово тот же текст Debug (объём, цена, маршрут, затраты; время котировки at — не итог, не сравнивается)
//! — на нескольких объёмах подряд одного короба (вторая и следующие точки — из короба), покупка и продажа, режимы
//! classic и graph. Путь — как в Quoting::priced: cpu::run(routing::run) (короб переносится в поток счёта).
//! Решение 363 (в логику): смена настроек внутри одного короба (поколение настроек) — допуск заново, котировка та же,
//! что без короба (settings_change_seen_within_share).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{cpu, routing}, arb::gateway::calc_share, tests/route_common.

mod route_common;

use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, Quote, Side, TokenTaxes};
use arb::gateway::calc_share::{self, Share};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::quote::{Quoting, cpu, routing};

use route_common::{CAKE, USDT};

/// Входные данные выбора маршрута (как их собирает quote/mod.rs).
fn input(q: &Quoting, side: Side, qty: u32) -> routing::Input {
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
        side,
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

/// Котировка через поток счёта; с коробом — под ним (как точка подбора объёма ядра). Текст Debug или ошибка.
async fn quote(q: &Quoting, side: Side, qty: u32, share: Option<Arc<Share>>) -> String {
    let work = cpu::run(
        &q.ctx,
        Box::pin(routing::run(q.clone(), input(q, side, qty))),
    );
    let out: Result<Quote, _> = match share {
        Some(s) => calc_share::scope(s, work).await,
        None => work.await,
    };
    match out {
        // Время котировки (at) — момент счёта, не итог: обнуляется для сравнения.
        Ok(mut v) => {
            v.at = chrono::DateTime::<chrono::Utc>::MIN_UTC;
            format!("{v:?}")
        }
        Err(e) => format!("ошибка: {}", e.message),
    }
}

/// Режим маршрута площадки (classic / graph) и прямой режим обмена.
async fn mode(q: &Quoting, router: &str) {
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("dex.router_mode", json!(router)),
        ("dex.swap_mode", json!("direct")),
    ] {
        q.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
}

/// С коробом (повторное использование) и без — та же котировка на каждом объёме, в обоих режимах и обе стороны.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reuse_same_quote_on_same_state() {
    let p = route_common::parts().await;
    let q = p.quoting();
    for router in ["classic", "graph"] {
        mode(&q, router).await;
        for side in [Side::Buy, Side::Sell] {
            // Один короб на все объёмы — как у одного подбора объёма.
            let share = Arc::new(Share::new(false, true));
            for qty in [10, 100, 1_000, 5_000, 20_000, 100] {
                let plain = quote(&q, side, qty, None).await;
                let reused = quote(&q, side, qty, Some(share.clone())).await;
                assert_eq!(plain, reused, "{router} {side:?} {qty}");
            }
        }
    }
}

/// Короб без повторного использования (только параллельные точки) — тоже та же котировка.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn par_only_share_same_quote() {
    let p = route_common::parts().await;
    let q = p.quoting();
    mode(&q, "graph").await;
    let share = Arc::new(Share::new(true, false));
    for qty in [10, 1_000, 20_000] {
        let plain = quote(&q, Side::Buy, qty, None).await;
        let shared = quote(&q, Side::Buy, qty, Some(share.clone())).await;
        assert_eq!(plain, shared, "{qty}");
    }
}

/// Решение 363 (в логику, правило 16.11): настройки изменились внутри одного подбора объёма (поколение настроек) — точка
/// того же короба допускает пулы и пути заново: котировка та же, что без короба. Здесь выключены все три DEX записи
/// (dex.pool_enabled = нет) — без проверки поколения короб отдал бы прежний допуск и прежний маршрут.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_change_seen_within_share() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let dexes = ["pancakeswap_v3", "uniswap_v3", "pancakeswap_v2"];
    for router in ["classic", "graph"] {
        mode(&q, router).await;
        enable(&q, &dexes, true).await;
        let share = Arc::new(Share::new(false, true));
        let before = quote(&q, Side::Buy, 1_000, None).await;
        assert_eq!(
            before,
            quote(&q, Side::Buy, 1_000, Some(share.clone())).await,
            "{router}: до смены"
        );
        enable(&q, &dexes, false).await;
        let plain = quote(&q, Side::Buy, 1_000, None).await;
        let reused = quote(&q, Side::Buy, 1_000, Some(share.clone())).await;
        assert_ne!(plain, before, "{router}: выключение DEX меняет котировку");
        assert_eq!(plain, reused, "{router}: после смены настроек");
    }
}

/// dex.pool_enabled для списка DEX (область «DEX»).
async fn enable(q: &Quoting, dexes: &[&str], on: bool) {
    for id in dexes {
        q.ctx
            .settings
            .set(
                "dex.pool_enabled",
                Scope::Dex((*id).into()),
                Some(json!(on)),
                "тест",
            )
            .await
            .unwrap();
    }
}
