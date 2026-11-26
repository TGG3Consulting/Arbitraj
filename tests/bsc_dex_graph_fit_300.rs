//! Файл: tests/bsc_dex_graph_fit_300.rs
//! Что делает: решение 300 — лучший исполнимый маршрут графа (quote/graph_fit.rs) на РЕАЛЬНЫХ записях CAKE
//! (tests/route_common, routes_2026-10-29 / 10-30; та же сцена, что gold-снимок tests/data/graph_shared_2026-11-08).
//! Сцена: режим graph, классика direct; smart.universal_router / smart.permit2 = адрес Universal Router строк
//! PancakeSwap — пути только по пулам PancakeSwap исполнимы одним UR (S1); путь через Uniswap V3 — только бандлом, а
//! канала бандла нет → такой лучший маршрут неисполним (как живой SIREN 15.11: «канал не в режиме бандла»).
//! Проверяется (выбор маршрута quote/routing.rs run, покупка/продажа 1…100000 CAKE, без газа и с газом):
//! (а) dex.graph_exec_fallback не задана / «нет» / «да» вне котировки решения (exec_quote = нет) / не задана при
//!     exec_quote = да — котировка 1 в 1 (JSON) как без настройки;
//! (б) «да» и exec_quote = да: лучший неисполним → на сделку исполнимый маршрут, не хуже классики (pick: объём, затем
//!     итог после газа); граф-победитель подписан в Quote.pick («S1 …»), неисполнимый — в signal_route, если лучше;
//!     хотя бы на одном объёме записи выигрывает исполнимый граф;
//! (в) лучший исполним — без повторов: с настоящим бандловым каналом (club48) лучший маршрут, в том числе с шагом
//!     Uniswap V3 (исполним только бандлом — проверяется, что такие есть), исполним; graph_route::quote_fit(fit = да)
//!     отдаёт Fit = None, котировка 1 в 1 с graph_route::quote.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/route_common, arb::venues::bsc_dex::quote::{routing, graph_route, pick}.

mod route_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{Asset, Quote, RouteMode, Side, TokenTaxes};
use arb::gateway::errors::GatewayResult;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, graph_route, pick, routing};

use route_common::{CAKE, USDT, WBNB};

const GAS: u64 = 1_000_000_000;
const DIRS: [&str; 2] = [
    "tests/data/routes_2026-10-29",
    "tests/data/routes_2026-10-30",
];
const QTYS: [u32; 7] = [1, 100, 1000, 5000, 20_000, 50_000, 100_000];

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

fn cake() -> Asset {
    Asset {
        chain: None,
        ticker: "CAKE".into(),
        contract: Some(CAKE),
    }
}

async fn set(p: &Parts, n: &str, v: Option<Value>) {
    let s = Scope::Venue("bsc_dex".into());
    p.ctx.settings.set(n, s, v, "тест").await.unwrap();
}

/// Сцена записи: режим graph, классика direct, умный UR = UR строк PancakeSwap (адрес route_common), учёт газа по флагу.
async fn scene(dir: &str, gas: bool) -> Parts {
    let p = route_common::parts_from(dir).await;
    let ur = json!("0x0000000000000000000000000000000000000099");
    set(&p, "dex.swap_mode", Some(json!("direct"))).await;
    set(&p, "dex.router_mode", Some(json!("graph"))).await;
    set(&p, "smart.universal_router", Some(ur.clone())).await;
    set(&p, "smart.permit2", Some(ur)).await;
    set(&p, "dex.native_wrapped", Some(json!(WBNB.to_string()))).await;
    if gas {
        set(&p, "dex.route_gas_aware", Some(json!(true))).await;
    }
    p
}

/// Выбор маршрута адаптера (routing::run) на CAKE. Ошибок нет — итог как есть.
async fn full(q: &Quoting, side: Side, qty: u32) -> GatewayResult<Quote> {
    let mids = q
        .mids
        .active(&q.ctx)
        .into_iter()
        .map(|m| (m, 18u8))
        .collect();
    let input = routing::Input {
        asset: cake(),
        side,
        qty: Decimal::from(qty),
        coin: CAKE,
        usdt: USDT,
        taxes: zero(),
        coin_dec: 18,
        usdt_dec: 18,
        entries: q.reg.by_pair(CAKE, USDT),
        mids,
        gas: U256::from(GAS),
        usdt_ticker: "USDT".into(),
        smart: false,
    };
    routing::run(q.clone(), input).await
}

/// Котировка в JSON без времени (сравнение «1 в 1»).
fn js(q: &GatewayResult<Quote>) -> Value {
    match q {
        Ok(q) => {
            let mut v = serde_json::to_value(q).unwrap();
            v.as_object_mut().unwrap().remove("at");
            v
        }
        Err(e) => json!({"code": format!("{:?}", e.code), "message": e.message}),
    }
}

/// Итог после газа в USDT (как pick.rs): продажа — больше лучше, покупка — меньше лучше.
async fn net(q: &Quoting, x: &Quote, side: Side) -> Decimal {
    let g = pick::gas_usdt(q, x, 18).await.unwrap_or(Decimal::ZERO);
    let v = x.avg_price * x.fillable_qty;
    if side == Side::Sell { v - g } else { v + g }
}

#[tokio::test]
async fn off_is_unchanged_on_picks_executable_not_worse_than_classic() {
    let mut won = 0;
    for dir in DIRS {
        for gas in [false, true] {
            let p = scene(dir, gas).await;
            let q = p.quoting();
            let mut qe = p.quoting();
            qe.exec_quote = true;
            for side in [Side::Buy, Side::Sell] {
                for qty in QTYS {
                    let tag = format!("{dir} gas={gas} {side:?} {qty}");
                    set(&p, "dex.graph_exec_fallback", None).await;
                    let base = js(&full(&q, side, qty).await);
                    assert_eq!(
                        js(&full(&qe, side, qty).await),
                        base,
                        "{tag}: exec_quote без настройки"
                    );
                    set(&p, "dex.graph_exec_fallback", Some(json!(false))).await;
                    assert_eq!(js(&full(&qe, side, qty).await), base, "{tag}: «нет»");
                    set(&p, "dex.graph_exec_fallback", Some(json!(true))).await;
                    assert_eq!(
                        js(&full(&q, side, qty).await),
                        base,
                        "{tag}: «да» вне котировки решения"
                    );
                    // (б) «да» на котировке решения.
                    let on = full(&qe, side, qty).await;
                    let Ok(on) = on else {
                        assert_eq!(js(&on), base, "{tag}: ошибка — та же");
                        continue;
                    };
                    let r = on.route.as_ref().unwrap();
                    assert!(r.executable, "{tag}: на сделку — только исполнимый");
                    set(&p, "dex.router_mode", Some(json!("classic"))).await;
                    let classic = full(&q, side, qty).await.unwrap();
                    set(&p, "dex.router_mode", Some(json!("graph"))).await;
                    assert!(on.fillable_qty >= classic.fillable_qty, "{tag}: объём");
                    if on.fillable_qty == classic.fillable_qty {
                        let (a, b) = (net(&q, &on, side).await, net(&q, &classic, side).await);
                        let ok = if side == Side::Sell { a >= b } else { a <= b };
                        assert!(ok, "{tag}: итог {a} хуже классики {b}");
                    }
                    if js(&Ok(on.clone())) != base {
                        // Изменилось только там, где выиграл исполнимый граф (подпись выбора и подмножество).
                        assert_eq!(r.mode, RouteMode::Graph, "{tag}");
                        let mixed =
                            arb::gateway::quote_stage::settlement_if(true, full(&qe, side, qty))
                                .await
                                .unwrap();
                        let route = mixed.route.as_ref().unwrap();
                        assert!(
                            route.executable
                                && matches!(route.mode, RouteMode::Direct | RouteMode::Smart)
                        );
                        assert_eq!(
                            (mixed.fillable_qty, mixed.avg_price),
                            (classic.fillable_qty, classic.avg_price),
                            "{tag}: разрешённый AMM при лучшем графе"
                        );
                        assert!(!arb::gateway::quote_stage::settlement());
                        let note = on.pick.clone().unwrap_or_default();
                        assert!(
                            note.contains("лучший исполнимый маршрут графа (S1"),
                            "{tag}: {note}"
                        );
                        won += 1;
                        println!("{tag}: {note}; signal_route={}", on.signal_route.is_some());
                    }
                }
            }
        }
    }
    assert!(
        won > 0,
        "на записях ни разу не выиграл исполнимый граф — тест ничего не проверил"
    );
    println!("исполнимый граф выиграл в {won} котировках");
}

#[tokio::test]
async fn executable_best_has_no_fit() {
    let (mut seen, mut uni) = (0, 0);
    for dir in DIRS {
        let p = scene(dir, false).await;
        // Настоящий бандловый канал (send/channels ALL — club48, как в других тестах бандла): send::planned_bundle_ok
        // = Ok. В route_common UR строк PancakeSwap и Uniswap — один адрес, поэтому путь через Uniswap V3 собирается в
        // бандл (collectable: один роутер ≤ dex.bundle_max_routers) — лучший маршрут с долей Uniswap исполним как есть.
        route_common::channel(&p, "club48", "bundle").await;
        set(&p, "dex.bundle_max_routers", Some(json!(2))).await;
        set(&p, "dex.graph_exec_fallback", Some(json!(true))).await;
        let mut q = p.quoting();
        q.exec_quote = true;
        let t = zero();
        for side in [Side::Buy, Side::Sell] {
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
                gas: "BNB",
            };
            for qty in QTYS {
                let (a, b) = (Decimal::from(qty), U256::from(GAS));
                let Ok((g, fit)) = graph_route::quote_fit(&v, &names, &cake(), a, b, true).await
                else {
                    continue;
                };
                if g.route.as_ref().is_some_and(|r| r.executable) {
                    // Маршрут с шагом Uniswap V3 — исполним только бандлом (не один UR PancakeSwap): тот самый случай.
                    // Смесь Uniswap V3 и PancakeSwap в одном маршруте — не прямая форма и не один UR PancakeSwap.
                    let via_uni = g.route.as_ref().is_some_and(|r| {
                        let fams: Vec<&str> = r
                            .legs
                            .iter()
                            .flat_map(|l| &l.hops)
                            .map(|h| h.family.as_str())
                            .collect();
                        fams.contains(&"uniswap_v3") && fams.iter().any(|f| *f != "uniswap_v3")
                    });
                    if via_uni {
                        uni += 1;
                    }
                    assert!(
                        fit.is_none(),
                        "{dir} {side:?} {qty}: исполнимый лучший — без повторов"
                    );
                    let plain = graph_route::quote(&v, &names, &cake(), a, b).await;
                    assert_eq!(js(&Ok(g)), js(&plain), "{dir} {side:?} {qty}");
                    seen += 1;
                }
            }
        }
    }
    assert!(
        seen > 0,
        "нет ни одного исполнимого лучшего маршрута на записях"
    );
    assert!(
        uni > 0,
        "нет ни одного исполнимого бандлом лучшего маршрута с шагом Uniswap V3 — тест не проверил бандловый случай"
    );
    println!("исполнимый лучший: {seen} котировок, из них с шагом Uniswap V3 (бандл): {uni}");
}
