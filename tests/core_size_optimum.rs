//! Файл: tests/core_size_optimum.rs
//! Что делает: подбор объёма (решение 66, core/size_search.rs) на записанных стакане BingX и пуле CAKE/USDT V2
//! (tests/size_common): списание по худшей цене не больше бюджета на любом бюджете в обе стороны; оптимум не хуже
//! перебора (каждый шаг монеты на малом бюджете, каждая целая монета и уровни стакана на большом); случай «бюджет /
//! лучшая цена» со списанием чуть больше бюджета (живой отказ WFI bsc_dex → bingx: «вложение 20.0000003 больше
//! бюджета 20») — прежний подбор отказывает, новый находит объём.
//! Параметры поиска — предлагаемые значения настроек size.* (решение 66); exec.dex_slippage_pct теста — 1 %.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::size*, tests/size_common.

mod size_common;

use arb::core::size::Curve;
use arb::core::size_search::search;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// Кривая с допуском 1 %.
fn curve(rec: &Rec, dir: Dir) -> RecCurve<'_> {
    RecCurve {
        rec,
        dir,
        slip: Decimal::ONE,
    }
}

/// Прибыль на объём, если списание в бюджете.
async fn profit_at(c: &mut RecCurve<'_>, q: Decimal, budget: Decimal) -> Option<Decimal> {
    match c.at(q).await {
        Ok(p) if p.invested <= budget => Some(p.profit),
        _ => None,
    }
}

/// Живой случай WFI на записях CAKE: «бюджет / лучшая цена» даёт списание чуть больше бюджета.
#[tokio::test]
async fn budget_edge_like_wfi() {
    let rec = Rec::load("edge").await;
    let budget = d("20");
    let p = params(&rec, Dir::DexToBingx, budget);
    let mut c = curve(&rec, Dir::DexToBingx);
    // Прежний подбор (profit::sizes до решения 66) начинал с этого объёма и отказывал (break).
    let first = c.at((p.guess / p.step).floor() * p.step).await.unwrap();
    assert!(
        first.invested > budget,
        "запись должна повторять живой случай: {}",
        first.invested
    );
    let f = search(&mut c, &p).await.unwrap();
    assert!(f.best.invested <= budget && f.choice.max_exact);
    let next = c.at(f.choice.max_qty + p.step).await.unwrap();
    assert!(next.invested > budget, "наибольший объём точный до шага");
    let mut q = p.step;
    while q <= f.choice.max_qty {
        if let Some(v) = profit_at(&mut c, q, budget).await {
            assert!(f.best.profit >= v, "объём {q}: {v} > {}", f.best.profit);
        }
        q += p.step;
    }
    println!("WFI-подобный случай: {:?}", f.choice);
}

/// Большой бюджет: оптимум внутри отрезка (проскальзывание пула против уровней bids), не хуже перебора.
#[tokio::test]
async fn interior_optimum_dex_to_bingx() {
    let rec = Rec::load("interior").await;
    let budget = d("20000");
    let p = params(&rec, Dir::DexToBingx, budget);
    let mut c = curve(&rec, Dir::DexToBingx);
    let f = search(&mut c, &p).await.unwrap();
    assert!(f.best.invested <= budget);
    assert!(f.choice.qty < f.choice.max_qty && f.choice.at_budget_profit < f.choice.profit);
    let mut grid: Vec<Decimal> = (1..=10000).map(Decimal::from).collect();
    for b in &p.sell_breaks {
        grid.extend([*b - p.step, *b, *b + p.step]);
    }
    let mut brute = Decimal::MIN;
    for q in grid {
        if let Some(v) = profit_at(&mut c, q, budget).await {
            brute = brute.max(v);
        }
    }
    assert!(
        f.best.profit >= brute,
        "оптимум {} хуже перебора {brute}",
        f.best.profit
    );
    println!(
        "CAKE bsc_dex → bingx, бюджет {budget}: {:?}, перебор {brute}",
        f.choice
    );
}

/// Инвариант: на любом бюджете списание не больше бюджета (или отказ), объём кратен шагу, в обе стороны.
#[tokio::test]
async fn spend_never_above_budget() {
    let rec = Rec::load("invariant").await;
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        for b in [
            "1",
            "5",
            "6",
            "20",
            "100",
            "1000",
            "5000",
            "20000",
            "100000",
            "1000000000",
        ] {
            let budget = d(b);
            let p = params(&rec, dir, budget);
            match search(&mut curve(&rec, dir), &p).await {
                Ok(f) => {
                    assert!(f.best.invested <= budget, "{dir:?} {b}");
                    assert!(
                        (f.choice.qty % p.step).is_zero(),
                        "{dir:?} {b}: {}",
                        f.choice.qty
                    );
                    assert!(f.choice.evals <= p.max_evals);
                    println!(
                        "{dir:?} {b}: объём {} прибыль {} расчётов {}",
                        f.choice.qty,
                        f.choice.profit.round_dp(4),
                        f.choice.evals
                    );
                }
                Err(e) => println!("{dir:?} {b}: отказ — {e}"),
            }
        }
    }
}
