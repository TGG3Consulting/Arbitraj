//! Файл: tests/core_presell.rs
//! Что делает: решение 234 — пересчёт плана по выходу симуляции продажи DEX до покупки (core/precheck_dex_sell.rs
//! apply, model_tax_pct) на настоящем расчёте ядра по записанным данным CAKE (tests/size_common/calc.rs: asks BingX
//! 2026-10-23, пул CAKE/USDT V2 на блоке 0x7b936ae, адаптер DEX quote/pool_quote::swap): симуляция дала столько же,
//! сколько план, — прибыль та же; меньше — прибыль меньше ровно на разницу, средняя продажи = выход / количество;
//! прибыль по симуляции ниже порога (USDT или %) — отказ с причиной; строка SellTax стороны B — модельный налог.
//! Записанного состояния сети для ASS (0x7c63…fbf0) в tests/data нет — сама eth_simulateV1 здесь не прогоняется.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{precheck_dex_sell, costs}, tests/size_common. Моков нет.

mod size_common;

use arb::core::costs::CostLine;
use arb::core::precheck_dex_sell::{apply, model_tax_pct};
use arb::core::types::{CostKind, DexPresell};
use rust_decimal::Decimal;
use size_common::{Rec, d};

/// Итог симуляции с выходом out (остальное — для текста журнала).
fn sim(out: Decimal, coins: Decimal) -> DexPresell {
    DexPresell {
        wallet: "0x0".into(),
        form: "прямой".into(),
        block: 0x7b936ae,
        coins,
        out,
        expected: out,
        min_legs: vec![],
        funding: None,
        gas_swap: 0,
    }
}

#[tokio::test]
async fn same_out_keeps_profit() {
    let rec = Rec::load("presell-same").await;
    let plan = size_common::calc::calc(&rec, d("10"));
    let s = sim(plan.proceeds, plan.sell_qty);
    let c =
        apply(plan.clone(), &s, plan.profit_usd, plan.profit_pct).expect("порог = прибыль плана");
    assert_eq!(c.profit_usd, plan.profit_usd);
    assert_eq!(c.proceeds, plan.proceeds);
}

#[tokio::test]
async fn lower_out_cuts_profit_by_difference() {
    let rec = Rec::load("presell-lower").await;
    let plan = size_common::calc::calc(&rec, d("10"));
    let cut = d("0.0123");
    let s = sim(plan.proceeds - cut, plan.sell_qty);
    let floor = plan.profit_usd - d("1");
    let c = apply(plan.clone(), &s, floor, d("-100")).expect("выше порога");
    assert_eq!(c.profit_usd, plan.profit_usd - cut);
    assert_eq!(c.proceeds, plan.proceeds - cut);
    assert_eq!(c.sell_avg, (plan.proceeds - cut) / plan.sell_qty);
    assert_eq!(
        c.profit_pct,
        c.profit_usd / plan.invested * Decimal::ONE_HUNDRED
    );
}

#[tokio::test]
async fn below_threshold_rejects() {
    let rec = Rec::load("presell-reject").await;
    let plan = size_common::calc::calc(&rec, d("10"));
    // Выход меньше выручки плана на прибыль плана и ещё 0.01 — прибыль по симуляции −0.01.
    let s = sim(plan.proceeds - plan.profit_usd - d("0.01"), plan.sell_qty);
    let e = apply(plan.clone(), &s, Decimal::ZERO, d("-100")).expect_err("ниже порога USDT");
    assert!(e.contains("невыгодна по симуляции продажи"), "{e}");
    let s = sim(plan.proceeds, plan.sell_qty);
    let e =
        apply(plan.clone(), &s, d("-1000"), plan.profit_pct + d("1")).expect_err("ниже порога %");
    assert!(e.contains('%'), "{e}");
}

#[tokio::test]
async fn model_tax_from_sell_tax_lines() {
    let rec = Rec::load("presell-tax").await;
    let mut plan = size_common::calc::calc(&rec, d("10"));
    assert_eq!(model_tax_pct(&plan), Decimal::ZERO);
    // Строка налога продажи стороны B в монете: 0.3166 CAKE от 10 — 3.166 % (число из плана ASS 08.11).
    plan.costs.push(CostLine {
        kind: CostKind::SellTax,
        stage: "B",
        amount: d("0.3166"),
        asset: "CAKE".into(),
        usdt: None,
        in_price: true,
        in_qty: false,
        step: None,
    });
    assert_eq!(model_tax_pct(&plan), d("3.166"));
}
