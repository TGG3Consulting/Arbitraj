//! Файл: tests/core_uncapped_peaks.rs
//! Что делает: расчёт «без потолка» (решение 141, core/size_uncapped.rs) на записанных данных CAKE (tests/size_common):
//! многопиковый случай — поиск уточняет больше одного пика и находит глобальный оптимум не хуже перебора целых монет;
//! замер стоимости — число полных расчётов кривой и мс на один расчёт при разных uncapped.max_evals.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{size_uncapped, size_search}, tests/size_common.

mod size_common;

use std::time::Instant;

use arb::core::size::{Curve, Fail, Point};
use arb::core::size_search::search;
use arb::core::size_uncapped::uncapped;
use async_trait::async_trait;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// Кривая с допуском 1 % (как в тестах подбора объёма).
fn curve(rec: &Rec, dir: Dir) -> RecCurve<'_> {
    RecCurve {
        rec,
        dir,
        slip: Decimal::ONE,
    }
}

/// Таблица потолков теста.
fn ladder() -> Vec<Decimal> {
    ["20", "50", "100", "500", "1000", "5000", "20000"]
        .iter()
        .map(|s| d(s))
        .collect()
}

/// Многопиковая кривая из записанной кривой CAKE DEX → BingX (естественной многопиковости в записи CAKE нет — кривая
/// вогнута): объём до `split` — одна сделка на записях; больше — сделка на `split` и остаток второй сделкой на тех же
/// записях (числа — только из записи, ничего не выдумано). За оптимумом первой части прибыль падает до `split`, потом
/// растёт вторая часть — два локальных максимума, второй выше, если прибыль на `split` > 0.
struct TwoPeaks<'a> {
    a: RecCurve<'a>,
    b: RecCurve<'a>,
    split: Decimal,
}

#[async_trait]
impl Curve for TwoPeaks<'_> {
    type Data = ();

    async fn at(&mut self, q: Decimal) -> Result<Point<()>, Fail> {
        if q <= self.split {
            return self.a.at(q).await;
        }
        let a = self.a.at(self.split).await?;
        let b = self.b.at(q - self.split).await?;
        Ok(Point {
            qty: q,
            invested: a.invested + b.invested,
            arrive: a.arrive + b.arrive,
            profit: a.profit + b.profit,
            data: (),
        })
    }
}

#[tokio::test]
async fn multi_peak_search_finds_global() {
    let rec = Rec::load("uncapped-peaks").await;
    let dir = Dir::DexToBingx;
    // Оптимум одной сделки — вершина первого пика; вторая сделка начинается за ним (на 30 % дальше оптимума).
    let one = search(&mut curve(&rec, dir), &params(&rec, dir, d("1000000")))
        .await
        .unwrap();
    let split = (one.best.qty * d("1.3")).floor();
    let mut c = TwoPeaks {
        a: curve(&rec, dir),
        b: curve(&rec, dir),
        split,
    };
    let p = params(&rec, dir, d("1000000"));
    let u = uncapped(&mut c, &p, d("1000000"), &[], |_| true)
        .await
        .unwrap();
    // Перебор целых монет до наибольшего объёма.
    let mut brute = Decimal::MIN;
    let mut x = Decimal::ONE;
    while x <= u.upper_qty {
        if let Ok(pt) = c.at(x).await {
            brute = brute.max(pt.profit);
        }
        x += Decimal::ONE;
    }
    println!(
        "пиков {}, оптимум {} на {}, перебор {brute}, расчётов {}",
        u.peaks, u.opt.profit, u.opt.qty, u.evals
    );
    assert!(u.peaks >= 2, "кривая с двумя вершинами: {}", u.peaks);
    assert!(u.opt.profit >= brute, "{} < {brute}", u.opt.profit);
    // Оптимум — на втором пике (за split), а не у края первого.
    assert!(u.opt.qty > split, "{} ≤ {split}", u.opt.qty);
}

/// Замер стоимости: сколько полных расчётов кривой и мс уходит на один расчёт без потолка при разных пределах
/// uncapped.max_evals и насколько итог отходит от предела 300 (числа — в docs/notes/uncapped-2026-11-04.md).
#[tokio::test]
async fn cost_by_max_evals() {
    let rec = Rec::load("uncapped-cost").await;
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        let mut full = None;
        for n in [300u32, 160, 120, 80] {
            let mut p = params(&rec, dir, d("1000000"));
            p.max_evals = n;
            let t = Instant::now();
            let u = uncapped(&mut curve(&rec, dir), &p, d("100"), &ladder(), |_| true)
                .await
                .unwrap();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let best = *full.get_or_insert(u.opt.profit);
            assert!(u.evals <= n);
            println!(
                "{dir:?} max_evals {n}: расчётов {}, {ms:.2} мс ({:.1} мкс на расчёт), оптимум {} (от предела 300: {}), предел исчерпан: {}",
                u.evals,
                ms * 1000.0 / f64::from(u.evals.max(1)),
                u.opt.profit.round_dp(4),
                (u.opt.profit - best).round_dp(6),
                u.capped
            );
        }
    }
}
