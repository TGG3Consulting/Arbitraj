//! Файл: tests/bsc_dex_bound_quote_t162.rs
//! Что проверяет: ревью t162, Н5 и Н8 — строгий верх курса стороны DEX (quote/bound.rs bound_now, тот же код, что
//! rate_bound, без проверки свежести головы — у записи её нет) против НАСТОЯЩЕЙ котировки адаптера на записанных пулах
//! CAKE (tests/route_common, routes_2026-10-29: шесть пулов V2 и Pancake/Uniswap V3 на одном блоке, промежуточные WBNB и
//! BTCB, в т. ч. пул между промежуточными BTCB–WBNB):
//! - классика «прямо» (direct_route: лучший пул или пара через промежуточную), умный режим (smart_quote: доли по пулам
//!   PancakeSwap и пары через промежуточные против прямой сделки Uniswap), граф (graph_route, dex.route_max_hops = 2:
//!   пути, сплит по DEX) — на объёмах от 1 до 50 000 CAKE в обе стороны;
//! - свойство: продажа — средняя цена котировки (USDT за CAKE) ≤ ρ_продажи; покупка — монет за USDT (1 / средняя) ≤
//!   ρ_покупки, т. е. средняя·ρ_покупки ≥ 1;
//! - путь через промежуточную (via.rs calc — тот же расчёт, что у классики и долей умного режима) — каждая пара пулов
//!   через WBNB и BTCB: монет/USDT пути не лучше верха;
//! - покрытие: путей через промежуточную > 0, сплитов (≥ 2 долей) > 0;
//! - Н8: у классики и умного режима ни одна доля не длиннее via.rs CLASSIC_MAX_HOPS (тест упадёт, если классика станет
//!   длиннее, а граница — нет);
//! - граф с dex.route_max_hops = 3 и пулом между промежуточными — метод не действует (класс mid_pool), не отсекает.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{bound, direct_route, smart_quote, graph_route, via, steps},
//! tests/route_common.

mod route_common;

use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, CheckState, CoinCheck, Quote, Side, TokenTaxes, VenueId};
use arb::gateway::rate_bound::RateBound;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::bound::bound_now;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::{CLASSIC_MAX_HOPS, Via};
use arb::venues::bsc_dex::quote::{Quoting, direct_route, graph_route, smart_quote};

use route_common::{BTCB, CAKE, USDT, WBNB};

/// Цена газа котировки, wei (как tests/bsc_dex_router_mode.rs).
const GAS: u64 = 1_000_000_000;

/// Адрес роутера и Permit2 строк DEX записи (route_common dex_row) — умный режим берёт пулы DEX с теми же адресами.
const ROUTER: &str = "0x0000000000000000000000000000000000000099";

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Настройка площадки bsc_dex. Ошибок нет (паника теста).
async fn set(p: &Parts, name: &str, v: serde_json::Value) {
    p.ctx
        .settings
        .set(name, Scope::Venue("bsc_dex".into()), Some(v), "тест")
        .await
        .unwrap();
}

/// Налоги промежуточных монет проверены и нулевые — путь через промежуточную допущен (via_pools.rs mid_ok; как
/// tests/bsc_dex_multihop.rs). Ошибок нет.
fn mids_checked(p: &Parts) {
    for (t, a) in [("BNB", WBNB), ("BTCB", BTCB)] {
        p.checks.set_coin(CoinCheck {
            venue: VenueId("bsc_dex".into()),
            asset: Asset {
                ticker: t.into(),
                bsc_contract: Some(a),
            },
            taxes: CheckState::Known(zero()),
            source: "тест".into(),
            checked_at: Some(Utc::now()),
            wait: None,
        });
    }
}

/// Путь через промежуточную (монет, USDT) не лучше верха (как check). Принимает: сторону, монет, USDT, ρ.
fn check_path(side: Side, coins: Decimal, usdt: Decimal, rho: Decimal) {
    match side {
        Side::Sell => assert!(
            usdt <= rho * coins,
            "путь через промежуточную: {usdt} USDT за {coins} > ρ {rho}"
        ),
        Side::Buy => assert!(
            coins <= rho * usdt,
            "путь через промежуточную: {coins} монет за {usdt} > ρ {rho}"
        ),
    }
}

/// Верх курса стороны (в целых монетах: decimals записи 18/18). Ошибок нет (паника — метод не действует).
fn rho(p: &Parts, side: Side) -> Decimal {
    match bound_now(p, &cake(), side) {
        RateBound::Bound(b) => b.rho,
        RateBound::NotApplicable { class, why } => panic!("верх {side:?}: {class}: {why}"),
    }
}

/// Котировка не лучше верха: продажа — средняя ≤ ρ; покупка — средняя·ρ ≥ 1. Принимает: что, сторону, котировку, ρ.
fn check(what: &str, side: Side, q: &Quote, rho: Decimal) {
    assert!(q.avg_price > Decimal::ZERO, "{what}: средняя ≤ 0");
    match side {
        Side::Sell => assert!(
            q.avg_price <= rho,
            "{what} {}: средняя продажи {} > ρ {rho}",
            q.qty,
            q.avg_price
        ),
        Side::Buy => assert!(
            q.avg_price * rho >= Decimal::ONE,
            "{what} {}: монет за USDT {} > ρ {rho}",
            q.qty,
            Decimal::ONE / q.avg_price
        ),
    }
}

/// Наибольшее число хопов доли и число долей котировки. Ошибок нет.
fn shape(q: &Quote) -> (usize, usize) {
    let r = q.route.as_ref().expect("у котировки DEX есть маршрут");
    let hops = r.legs.iter().map(|l| l.hops.len()).max().unwrap_or(0);
    (hops, r.legs.len())
}

/// Объёмы проверки, CAKE.
fn sizes() -> Vec<Decimal> {
    [1u32, 10, 100, 1_000, 5_000, 20_000, 50_000]
        .into_iter()
        .map(Decimal::from)
        .collect()
}

/// Все котировки трёх режимов по сторонам и объёмам против верха. Возвращает: (котировок, через промежуточную, сплитов).
async fn all_modes(p: &Parts) -> (usize, usize, usize) {
    let q: Quoting = p.quoting();
    let t = zero();
    let mids: Vec<(Mid, u8)> = q.mids.active(&q.ctx).into_iter().map(|m| (m, 18)).collect();
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    };
    let (mut n, mut via_mid, mut split) = (0, 0, 0);
    for side in [Side::Sell, Side::Buy] {
        let r_classic = rho(p, side);
        let v = Via {
            q: &q,
            coin: CAKE,
            usdt: USDT,
            side,
            taxes: &t,
            coin_dec: 18,
            usdt_dec: 18,
        };
        for qty in sizes() {
            let entries: Vec<Entry> = q
                .reg
                .by_pair(CAKE, USDT)
                .into_iter()
                .filter(|e| e.ticker == "CAKE")
                .collect();
            let keep = |_: &Entry| true;
            let gas = U256::from(GAS);
            let direct =
                direct_route::quote(&v, &names, &cake(), qty, entries.clone(), &mids, &keep, gas)
                    .await;
            let smart = smart_quote::quote(&v, &names, &cake(), qty, entries, &mids, gas).await;
            // Каждая пара пулов через каждую промежуточную (via.rs calc — тот же расчёт, что у классики и умного режима).
            for (m, dec) in &mids {
                let Ok((p1s, p2s)) = v.pools(m, &|_: &Entry| true) else {
                    continue;
                };
                for p1 in &p1s {
                    for p2 in &p2s {
                        let Ok(path) = v.calc(m, *dec, p1, p2, qty) else {
                            continue;
                        };
                        check_path(side, path.coins, path.usdt, r_classic);
                        via_mid += 1;
                    }
                }
            }
            for (what, got) in [("классика", direct), ("умный", smart)] {
                let Ok(quote) = got else { continue };
                check(what, side, &quote, r_classic);
                let (hops, legs) = shape(&quote);
                assert!(
                    hops <= CLASSIC_MAX_HOPS,
                    "{what}: доля из {hops} хопов > CLASSIC_MAX_HOPS"
                );
                n += 1;
                via_mid += usize::from(hops == 2);
                split += usize::from(legs >= 2);
            }
        }
    }
    // Граф: dex.router_mode = graph, предел 2 хопа (пул между промежуточными верх не ломает при H ≤ 2).
    set(p, "dex.router_mode", json!("graph")).await;
    set(p, "dex.route_max_hops", json!(2)).await;
    for side in [Side::Sell, Side::Buy] {
        let r_graph = rho(p, side);
        let v = Via {
            q: &q,
            coin: CAKE,
            usdt: USDT,
            side,
            taxes: &t,
            coin_dec: 18,
            usdt_dec: 18,
        };
        for qty in sizes() {
            let Ok(quote) = graph_route::quote(&v, &names, &cake(), qty, U256::from(GAS)).await
            else {
                continue;
            };
            check("граф", side, &quote, r_graph);
            let (hops, legs) = shape(&quote);
            n += 1;
            via_mid += usize::from(hops == 2);
            split += usize::from(legs >= 2);
        }
    }
    (n, via_mid, split)
}

/// Н5, Н8: настоящие котировки классики, умного режима и графа на записанных пулах — не лучше строгого верха.
#[tokio::test]
async fn real_quotes_never_beat_rate_bound() {
    let p = route_common::parts().await;
    mids_checked(&p);
    set(&p, "smart.universal_router", json!(ROUTER)).await;
    set(&p, "smart.permit2", json!(ROUTER)).await;
    let (n, via_mid, split) = Box::pin(all_modes(&p)).await;
    println!("котировок {n}, через промежуточную {via_mid}, сплитов {split}");
    assert!(n >= 20, "котировок мало: {n}");
    assert!(via_mid > 0, "ни одной котировки через промежуточную");
    assert!(split > 0, "ни одного сплита");
    // Граф до 3 хопов и пул между промежуточными (BTCB–WBNB) — метод не действует, отсева нет.
    set(&p, "dex.route_max_hops", json!(3)).await;
    match bound_now(&p, &cake(), Side::Sell) {
        RateBound::NotApplicable { class, .. } => assert_eq!(class, "mid_pool"),
        RateBound::Bound(b) => panic!("при 3 хопах и пуле между промежуточными верх {b:?}"),
    }
}
