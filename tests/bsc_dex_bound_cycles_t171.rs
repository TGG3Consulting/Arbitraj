//! Файл: tests/bsc_dex_bound_cycles_t171.rs
//! Что проверяет: решение 385 (задача t171) — офлайн-доказательство отсева циклов DEX ↔ DEX (решение 373,
//! venues/bsc_dex/quote/bound_cycle.rs) перебором ВСЕХ циклов записанного графа. Записи маршрута CAKE (рекордер
//! record_route.js; tests/route_common): routes_2026-10-29 (блок 130521896) и routes_2026-10-30 (блок 130556914) —
//! шесть настоящих пулов (Pancake V3 ×3, Uniswap V3 ×2, V2), граф бота (pools/graph.rs), циклы от каждого узла (USDT,
//! CAKE, WBNB, BTCB) длиной до 4 (route_paths::enumerate_cycles), ноги — тем же путём, что у поиска
//! (cycle::legs_for_cycle: допуск пулов select::usable). Для каждого цикла:
//! - верх ρ = cycle_rho; на сетке входов (10⁻⁶…5·10⁶ базовой) выход полного счёта (cycle::eval) ≤ ρ·вход — это и есть
//!   утверждение границы (из него ρ < 1 ⇒ выход < вход на любом входе);
//! - оптимальный вход (cycle_opt::best, как в поиске) на нескольких верхних границах: если прибыль до газа (выход − вход) больше
//!   нуля — граница обязана говорить «может быть прибылен» (cycle_may_profit = true). Прибыль до газа > 0 — шире условия
//!   ядра (чистая > 0, core/cycle_watch.rs): чистая = валовая − газ, газ ≥ 0.
//!
//! Тест падает на первом контрпримере с его распечаткой (маршрут, ρ, вход, выход). Ревью t171 (Н6) — тест не должен
//! проходить вхолостую: у каждого цикла с верхом посчитана хотя бы одна точка сетки (иначе падение с маршрутом);
//! реально отсечённых циклов (ρ < 1) больше нуля (иначе слишком слабая граница прошла бы тест); не все циклы взяли ноги
//! с явным допуском. Итог перебора — в сообщениях утверждений и в eprintln (виден с --nocapture).
//! Кто вызывает: cargo test --test bsc_dex_bound_cycles_t171.
//! От чего зависит: tests/route_common, arb::venues::bsc_dex::{pools::graph, quote::{bound_cycle, bound_rate, cycle,
//! cycle_opt, route_paths}}, arb::settings.

mod route_common;

use std::collections::HashMap;

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde_json::json;

use arb::settings::scope::Scope;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::bound_cycle::{cycle_may_profit, cycle_rho};
use arb::venues::bsc_dex::quote::bound_rate::mul_up;
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::select::Usable;
use arb::venues::bsc_dex::quote::{cycle, cycle_opt, route_paths};

use route_common::{BTCB, CAKE, USDT, WBNB};

/// Итог перебора одной записи.
#[derive(Default)]
struct Tally {
    cycles: usize,
    no_legs: usize,
    bounded: usize,
    no_bound: usize,
    cut: usize,
    points: usize,
    eval_err: usize,
    profitable: usize,
}

impl Tally {
    /// Итог одной строкой. Ошибок нет.
    fn line(&self) -> String {
        format!(
            "циклов {}, ноги с явным допуском (пул не допущен проверками записи) {}, с верхом ρ {}, без строгого верха {}, \
             отсечено (ρ < 1) {}, точек сетки {}, точек без счёта {}, прибыльных до газа (оптимум) {}",
            self.cycles,
            self.no_legs,
            self.bounded,
            self.no_bound,
            self.cut,
            self.points,
            self.eval_err,
            self.profitable
        )
    }
}

/// Маршрут цикла для распечатки: вид и адрес пула, токен входа каждого шага.
fn route(legs: &[Leg]) -> String {
    legs.iter()
        .map(|l| {
            format!(
                "[{} {:?} {:?} вход {}]",
                l.entry.family.id, l.entry.pool.kind, l.entry.pool.reference, l.token_in
            )
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Нога по шагу цикла с явным допуском (газ из записи не нужен — граница от газа не зависит; без крючка и налога) и
/// decimals 18 — как в tests/bsc_dex_cycle.rs.
fn leg_of(step: &route_paths::Step) -> Leg {
    Leg {
        entry: (*step.edge.entry).clone(),
        usable: Usable {
            gas: 0,
            hook_pct: Decimal::ZERO,
            tax_pct: Decimal::ZERO,
            hook: Default::default(),
        },
        token_in: step.token_in,
        token_out: step.token_out,
        dec_in: 18,
        dec_out: 18,
    }
}

/// Сетка входов базовой монеты: {1, 2, 5}·10^e, e = −6…6.
fn grid() -> Vec<Decimal> {
    let mut v = Vec::new();
    for e in 0u32..=12 {
        for m in [1i64, 2, 5] {
            let x = Decimal::new(m, 0) * Decimal::from(10i64.pow(e)) / Decimal::from(1_000_000i64);
            v.push(x);
        }
    }
    v
}

/// Перебор всех циклов записи `dir` (см. шапку). Паника — контрпример. Возвращает: итог.
async fn sweep(dir: &str) -> Tally {
    let p = route_common::parts_from(dir).await;
    // Шаги поиска оптимального входа — как у поиска циклов (size.refine_steps, решение 124).
    p.ctx
        .settings
        .set("size.refine_steps", Scope::Global, Some(json!(40)), "тест")
        .await
        .unwrap();
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    let q = p.quoting();
    let dec: HashMap<Address, u8> = [USDT, CAKE, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    let caps = [Decimal::ONE, Decimal::from(1_000), Decimal::from(1_000_000)];
    let mut t = Tally::default();
    for base in [USDT, CAKE, WBNB, BTCB] {
        for path in route_paths::enumerate_cycles(&g, base, 4, 10_000) {
            t.cycles += 1;
            // Ноги — как у поиска; пул не допущен проверками записи — та же математика с явным допуском (без
            // крючка и налога), чтобы граница проверялась на всех циклах графа.
            let legs = match cycle::legs_for_cycle(&q, &path, 18, &dec) {
                Ok(l) => l,
                Err(_) => {
                    t.no_legs += 1;
                    path.steps.iter().map(leg_of).collect()
                }
            };
            let Ok(rho) = cycle_rho(&p.ctx, &legs) else {
                t.no_bound += 1;
                continue;
            };
            t.bounded += 1;
            if rho < Decimal::ONE {
                t.cut += 1;
            }
            let mut mine = 0usize;
            for x in grid() {
                let Ok(r) = cycle::eval(&p.ctx, &legs, x) else {
                    // Вход больше, чем пулы исполнят целиком, — точки нет; считаем, у цикла должна быть хоть одна.
                    t.eval_err += 1;
                    continue;
                };
                t.points += 1;
                mine += 1;
                let cap = mul_up(rho, r.coins).expect("ρ·вход помещается в число");
                assert!(
                    r.usdt <= cap,
                    "{dir}: КОНТРПРИМЕР границы цикла: выход {} > ρ·вход {cap} (ρ = {rho}, вход {}); маршрут {}",
                    r.usdt,
                    r.coins,
                    route(&legs)
                );
            }
            assert!(
                mine > 0,
                "{dir}: у цикла с верхом ρ = {rho} не посчитана ни одна точка сетки — граница на нём не проверена; маршрут {}",
                route(&legs)
            );
            for c in caps {
                let Ok(b) = cycle_opt::best(&p.ctx, &legs, c) else {
                    continue;
                };
                if b.usdt - b.coins > Decimal::ZERO {
                    t.profitable += 1;
                    assert!(
                        cycle_may_profit(&p.ctx, &legs),
                        "{dir}: КОНТРПРИМЕР отсева цикла: прибыль до газа {} > 0 (вход {}, выход {}), а граница ρ = {rho} \
                         отсекает; маршрут {}",
                        b.usdt - b.coins,
                        b.coins,
                        b.usdt,
                        route(&legs)
                    );
                }
            }
        }
    }
    t
}

#[tokio::test]
async fn every_recorded_cycle_respects_bound() {
    for dir in [
        "tests/data/routes_2026-10-29",
        "tests/data/routes_2026-10-30",
    ] {
        let t = sweep(dir).await;
        eprintln!("t171 {dir}: {}", t.line());
        assert!(
            t.cycles > 0,
            "{dir}: в графе записи есть циклы: {}",
            t.line()
        );
        assert!(
            t.bounded > 0 && t.points > 0,
            "{dir}: перебор не пустой — есть циклы с верхом и посчитанные точки: {}",
            t.line()
        );
        assert!(
            t.cut > 0,
            "{dir}: граница отсекает хотя бы один цикл (ρ < 1) — иначе слишком слабая граница прошла бы тест: {}",
            t.line()
        );
        assert!(
            t.no_legs < t.cycles,
            "{dir}: не все циклы взяли ноги с явным допуском — путь поиска (select::usable) проверен: {}",
            t.line()
        );
    }
}
