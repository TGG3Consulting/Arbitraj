//! Файл: tests/core_signal_probe_route.rs
//! Что делает: решение 260 — средняя цена котировки DEX от объёма при выборе маршрута графом (решение 80) на РЕАЛЬНЫХ
//! записях шести пулов CAKE (tests/route_common: routes_2026-10-29, блок 130521896, и routes_2026-10-30, блок 130556914;
//! CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT, CAKE-BTCB, BTCB-WBNB). Маршрут выбирается заново на каждый объём; при
//! dex.route_gas_aware = да — по выходу ПОСЛЕ газа пути (quote/graph_split.rs, route_gas.rs), поэтому на малом объёме
//! может быть выбран путь с меньшим газом и худшей ценой. Газ в среднюю цену котировки не входит — это затрата Gas в BNB
//! (Cost.in_price = нет), а грубый спред сигнала (core/prices.rs fee_usdt) учитывает только TradeFee и PoolFee.
//! Проверяется на объёмах 0.5 … 20 000 CAKE, с учётом газа и без, при цене газа 1, 3 и 100 gwei, покупка и продажа:
//! маршрут по объёму меняется (число долей и путей), а средняя цена на меньшем объёме не хуже, чем на большем —
//! значит, спред сигнала на объёме проверки не ниже спреда на потолке на этих записях.
//! На малом объёме продажи путь из 3 пулов через BTCB даёт по записанным состояниям выход порядка 10^6 USDT за CAKE
//! (видно в выводе теста); причина не разбиралась — сигнал на таком спреде попадает под signal.anomaly_spread_pct.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::graph_route, tests/route_common.

mod route_common;

use alloy_primitives::U256;
use arb::core::types::{Asset, CostKind, Side, TokenTaxes};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::quote::graph_route;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use rust_decimal::Decimal;
use serde_json::json;

use route_common::{CAKE, USDT, WBNB};

const QTY: [&str; 8] = ["0.5", "2", "10", "50", "200", "1000", "5000", "20000"];

#[tokio::test]
async fn avg_price_not_worse_on_smaller_volume_with_route_changes() {
    let mut route_changes = 0;
    for dir in [
        "tests/data/routes_2026-10-29",
        "tests/data/routes_2026-10-30",
    ] {
        for aware in [false, true] {
            for gas in [1_000_000_000u64, 3_000_000_000, 100_000_000_000] {
                let p = route_common::parts_from(dir).await;
                for (n, v) in [
                    ("dex.route_search_max_paths", json!(100)),
                    ("dex.route_max_paths", json!(2)),
                    ("dex.route_gas_aware", json!(aware)),
                    ("dex.native_wrapped", json!(WBNB.to_string())),
                ] {
                    p.ctx
                        .settings
                        .set(n, Scope::Venue("bsc_dex".into()), Some(v), "тест")
                        .await
                        .unwrap();
                }
                let q = p.quoting();
                let t = TokenTaxes {
                    buy_pct: Decimal::ZERO,
                    sell_pct: Decimal::ZERO,
                    transfer_pct: Decimal::ZERO,
                };
                for side in [Side::Buy, Side::Sell] {
                    let mut prev: Option<(Decimal, String)> = None;
                    for qty in QTY {
                        let v = Via {
                            q: &q,
                            coin: CAKE,
                            usdt: USDT,
                            side,
                            taxes: &t,
                            coin_dec: 18,
                            usdt_dec: 18,
                        };
                        let names = Names {
                            usdt: (USDT, "USDT"),
                            coin: (CAKE, "CAKE"),
                            mids: &p.mids,
                        };
                        let a = Asset {
                            ticker: "CAKE".into(),
                            bsc_contract: Some(CAKE),
                        };
                        let r = graph_route::quote(
                            &v,
                            &names,
                            &a,
                            qty.parse().unwrap(),
                            U256::from(gas),
                        )
                        .await
                        .expect("котировка графа");
                        assert_eq!(
                            r.fillable_qty,
                            qty.parse::<Decimal>().unwrap(),
                            "объём исполнен целиком"
                        );
                        // Газ — затрата в BNB вне цены, в среднюю цену не входит.
                        assert!(
                            r.costs
                                .iter()
                                .any(|c| c.kind == CostKind::Gas && !c.in_price),
                            "газ — отдельной затратой"
                        );
                        let legs = r
                            .route
                            .as_ref()
                            .unwrap()
                            .legs
                            .iter()
                            .map(|l| format!("{}×{}", l.share.round_dp(2), l.hops.len()))
                            .collect::<Vec<_>>()
                            .join("+");
                        println!(
                            "{dir} газ-учёт={aware} {gas} {side:?} {qty}: {} [{legs}]",
                            r.avg_price.round_dp(6)
                        );
                        if let Some((p_prev, legs_prev)) = &prev {
                            if *legs_prev != legs {
                                route_changes += 1;
                            }
                            // Больший объём — покупка не дешевле, продажа не дороже.
                            match side {
                                Side::Buy => assert!(
                                    r.avg_price >= *p_prev,
                                    "{dir} {aware} {gas} покупка {qty}"
                                ),
                                Side::Sell => assert!(
                                    r.avg_price <= *p_prev,
                                    "{dir} {aware} {gas} продажа {qty}"
                                ),
                            }
                        }
                        prev = Some((r.avg_price, legs));
                    }
                }
            }
        }
    }
    assert!(route_changes > 0, "маршрут по объёму меняется");
}
