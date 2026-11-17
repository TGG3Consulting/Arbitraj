//! Файл: tests/bsc_dex_cycle_audit_t171.rs
//! Что проверяет: решение 385 — контроль отсева циклов DEX ↔ DEX (venues/bsc_dex/quote/cycle_audit.rs):
//! - накопитель доли sieve.audit_share без случайности (step): 0,1 — ровно каждый десятый, 0,3 — 30 из 100, не задана /
//!   0 / < 0 — никогда, > 1 — как 1 (каждый);
//! - нарушение только при ρ < 1 И прибыли > 0 (violated): на РЕАЛЬНОМ цикле записи routes_2026-10-29 (USDT → CAKE → USDT
//!   двумя пулами V3; tests/route_common) ρ считается на тех же ногах, что и сигнал (Leg несёт копию записи пула — тот же
//!   снимок), отсев (cycle_sieve) отдаёт тот же ρ; настоящий сигнал (выход < вход) — не нарушение; тот же сигнал с
//!   прибылью > 0 (чистая логика: поля заменены) — нарушение при ρ < 1 и не нарушение при ρ ≥ 1;
//! - check: нарушение — запись с маршрутом, ρ, входом, выходом; путь запоминается и дальше не отсекается (exempt).
//!
//! Кто вызывает: cargo test --test bsc_dex_cycle_audit_t171.
//! От чего зависит: tests/route_common, arb::venues::bsc_dex::quote::{bound_cycle, cycle, cycle_audit, route_paths},
//! pools::graph.

mod route_common;

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::bound_cycle::{CycleSieve, cycle_rho, cycle_sieve};
use arb::venues::bsc_dex::quote::cycle_audit::{check, exempt, key, report, step, violated};
use arb::venues::bsc_dex::quote::{cycle, route_paths};

use route_common::{BTCB, CAKE, USDT, WBNB};

/// Сколько из `n` отсечённых идут в контроль при доле `share`.
fn picked(share: Option<Decimal>, n: usize) -> Vec<bool> {
    let mut acc = Decimal::ZERO;
    (0..n).map(|_| step(&mut acc, share)).collect()
}

#[test]
fn share_accumulator_without_randomness() {
    let tenth = picked(Some(Decimal::new(1, 1)), 100);
    assert_eq!(tenth.iter().filter(|x| **x).count(), 10);
    assert!(
        tenth[9] && !tenth[..9].iter().any(|x| *x),
        "первый контроль — десятый"
    );
    let three = picked(Some(Decimal::new(3, 1)), 100);
    assert_eq!(three.iter().filter(|x| **x).count(), 30);
    for none in [None, Some(Decimal::ZERO), Some(Decimal::new(-1, 1))] {
        assert!(
            !picked(none, 50).iter().any(|x| *x),
            "нет доли — контроля нет"
        );
    }
    assert!(
        picked(Some(Decimal::TWO), 20).iter().all(|x| *x),
        "> 1 — как 1"
    );
}

#[tokio::test]
async fn violation_only_below_one_with_profit_same_snapshot() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    route_common::executor(&p, "0x00000000000000000000000000000000000000ee").await;
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    let q = p.quoting();
    let dec: HashMap<Address, u8> = [USDT, CAKE, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    let cyc = route_paths::enumerate_cycles(&g, USDT, 2, 100)
        .into_iter()
        .next()
        .expect("двухшаговый цикл USDT → CAKE → USDT");
    let legs = cycle::legs_for_cycle(&q, &cyc, 18, &dec).expect("ноги цикла");
    let rho = cycle_rho(&p.ctx, &legs).expect("верх цикла V3");
    assert!(
        rho < Decimal::ONE,
        "два пула с комиссией: ρ < 1 (ρ = {rho})"
    );
    assert_eq!(
        cycle_sieve(&p.ctx, &legs),
        CycleSieve::Cut(rho),
        "отсев отдаёт ρ тех же ног"
    );
    let path = cycle::eval(&p.ctx, &legs, Decimal::from(100)).expect("счёт цикла");
    let gas = U256::from(1_000_000_000u64);
    let sig = cycle::build_signal(
        &p.ctx,
        USDT,
        "USDT",
        &path,
        &legs,
        gas,
        Decimal::ZERO,
        Some(Decimal::from(600)),
    )
    .expect("сигнал");
    assert!(
        sig.gross_profit < Decimal::ZERO,
        "настоящий цикл: выход < вход"
    );
    assert!(!violated(rho, &sig), "настоящий сигнал — не нарушение");
    assert!(check(rho, &sig, key(&legs)).is_none());
    assert!(!exempt(&key(&legs)));

    // Чистая логика: тот же сигнал с прибылью > 0.
    let mut won = sig.clone();
    won.output = won.input + Decimal::ONE;
    won.gross_profit = Decimal::ONE;
    won.net_profit = Some(Decimal::new(5, 1));
    assert!(violated(rho, &won));
    assert!(
        !violated(Decimal::ONE, &won),
        "ρ ≥ 1 — граница прибыль не исключала"
    );
    let mut gross_only = won.clone();
    gross_only.net_profit = None;
    assert!(
        violated(rho, &gross_only),
        "валовая > 0 при ρ < 1 — уже нарушение"
    );
    let mut zero = won.clone();
    zero.gross_profit = Decimal::ZERO;
    zero.net_profit = Some(Decimal::ZERO);
    assert!(!violated(rho, &zero), "ноль — не прибыль");

    let r = report(rho, &won);
    assert!(r.text.contains(&rho.to_string()) && r.text.contains("USDT"));
    assert_eq!(r.data["kind"], "cycle");
    let v = check(rho, &won, key(&legs)).expect("нарушение");
    assert_eq!(v.data["kind"], "cycle");
    assert!(
        exempt(&key(&legs)),
        "путь с нарушением дальше не отсекается"
    );
}
