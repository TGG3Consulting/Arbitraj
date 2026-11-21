//! Файл: tests/core_uncapped.rs
//! Что делает: расчёт «без потолка» (решение 141, core/size_uncapped.rs) на записанных данных CAKE (tests/size_common:
//! стакан BingX ws_spot_2026-10-23, пул CAKE/USDT V2 на блоке 0x7b936ae; кривая — те же функции ядра и адаптеров, что в
//! тестах подбора объёма): без потолка не хуже, чем с потолком (и не хуже боевого подбора size_search на том же
//! бюджете); минимальное вложение до плюса совпадает с прямым расчётом кривой на этом объёме и на шаг меньше пороги не
//! проходит; при не ограничивающем бюджете упущено = 0; таблица потолков не убывает. Многопиковый случай и замер
//! стоимости — tests/core_uncapped_peaks.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{size_uncapped, size_search, uncapped_types}, tests/size_common.

mod size_common;

use std::time::Instant;

use arb::core::size::Curve;
use arb::core::size_search::search;
use arb::core::size_uncapped::uncapped;
use arb::core::uncapped_types::{missed, take};
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

#[tokio::test]
async fn uncapped_not_worse_than_capped() {
    let rec = Rec::load("uncapped-ge").await;
    for dir in [Dir::BingxToDex, Dir::DexToBingx] {
        for b in ["20", "100", "1000"] {
            let budget = d(b);
            let battle = search(&mut curve(&rec, dir), &params(&rec, dir, budget))
                .await
                .unwrap();
            let p = params(&rec, dir, d("1000000"));
            let t = Instant::now();
            let u = uncapped(&mut curve(&rec, dir), &p, budget, &ladder(), |_| true, None)
                .await
                .unwrap();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let at = u.at_budget.at.expect("на бюджете есть допустимый объём");
            assert!(at.invested <= budget, "{dir:?} {b}");
            assert!(u.opt.profit >= at.profit, "{dir:?} {b}");
            assert!(
                u.opt.profit >= battle.best.profit,
                "{dir:?} {b}: без потолка хуже боя"
            );
            // Таблица потолков не убывает с потолком.
            let takes: Vec<Decimal> = u.levels.iter().map(take).collect();
            assert!(takes.windows(2).all(|w| w[0] <= w[1]), "{dir:?}: {takes:?}");
            assert!(u.evals <= p.max_evals);
            println!(
                "{dir:?} бюджет {b}: без потолка {} USDT на {} (вложить {}), бой {}, на бюджете {}, расчётов {}, пиков {}, {ms:.2} мс; край: {}",
                u.opt.profit.round_dp(4),
                u.opt.qty,
                u.opt.invested.round_dp(2),
                battle.best.profit.round_dp(4),
                at.profit.round_dp(4),
                u.evals,
                u.peaks,
                u.limit
            );
        }
    }
}

#[tokio::test]
async fn min_plus_matches_direct_calc() {
    let rec = Rec::load("uncapped-min").await;
    for dir in [Dir::BingxToDex, Dir::DexToBingx] {
        // Порог — прибыль лучшего объёма бюджета 50: плюсовые объёмы заведомо есть.
        let small = search(&mut curve(&rec, dir), &params(&rec, dir, d("50")))
            .await
            .unwrap();
        let min = small.best.profit;
        let p = params(&rec, dir, d("1000000"));
        let mut c = curve(&rec, dir);
        let u = uncapped(&mut c, &p, d("50"), &[], |pt| pt.profit >= min, None)
            .await
            .unwrap();
        let m = u.min_plus.expect("есть плюсовой объём");
        let direct = c.at(m.qty).await.unwrap();
        assert_eq!(
            (direct.profit, direct.invested),
            (m.profit, m.invested),
            "{dir:?}"
        );
        assert!(direct.profit >= min, "{dir:?}");
        assert!(
            m.qty <= small.best.qty,
            "{dir:?}: минимум не больше заведомо проходящего"
        );
        let below = c.at(m.qty - p.step).await;
        assert!(
            below.map(|pt| pt.profit < min).unwrap_or(true),
            "{dir:?}: на шаг меньше — уже плюс"
        );
        println!(
            "{dir:?}: плюс (≥ {}) от {} монет, вложить {} USDT",
            min.round_dp(4),
            m.qty,
            m.invested.round_dp(4)
        );
    }
    // Порог выше любой прибыли — «не становится плюсовой».
    let p = params(&rec, Dir::BingxToDex, d("1000000"));
    let u = uncapped(
        &mut curve(&rec, Dir::BingxToDex),
        &p,
        d("50"),
        &[],
        |pt| pt.profit >= d("1000000000"),
        None,
    )
    .await
    .unwrap();
    assert!(u.min_plus.is_none() && !u.opt_passes);
}

#[tokio::test]
async fn missed_zero_when_budget_not_binding() {
    let rec = Rec::load("uncapped-zero").await;
    for dir in [Dir::BingxToDex, Dir::DexToBingx] {
        let p = params(&rec, dir, d("1000000"));
        let u = uncapped(&mut curve(&rec, dir), &p, d("1000000"), &[], |_| true, None)
            .await
            .unwrap();
        assert_eq!(u.at_budget.at.unwrap().profit, u.opt.profit, "{dir:?}");
        assert_eq!(
            missed("accept", true, u.opt.profit, &u.at_budget),
            Decimal::ZERO
        );
        assert_eq!(
            missed("deal", true, u.opt.profit, &u.at_budget),
            Decimal::ZERO
        );
    }
    // Ограничивающий бюджет — упущено > 0; отказ — вся прибыль без потолка (BingX → DEX на записи — минус на любом
    // объёме, поэтому — DEX → BingX).
    let dir = Dir::DexToBingx;
    let p = params(&rec, dir, d("1000000"));
    {
        let u = uncapped(&mut curve(&rec, dir), &p, d("20"), &[], |_| true, None)
            .await
            .unwrap();
        let m = missed("accept", true, u.opt.profit, &u.at_budget);
        assert_eq!(m, u.opt.profit - u.at_budget.at.unwrap().profit, "{dir:?}");
        assert!(m > Decimal::ZERO, "{dir:?}");
        assert_eq!(
            missed("reject", true, u.opt.profit, &u.at_budget),
            u.opt.profit
        );
        assert_eq!(
            missed("reject", false, u.opt.profit, &u.at_budget),
            Decimal::ZERO
        );
    }
}
