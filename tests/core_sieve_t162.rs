//! Файл: tests/core_sieve_t162.rs
//! Что проверяет: решения 372–374 — отсев кандидата по доказанной границе прибыли (core/bound.rs, core/bound_in.rs):
//! 1) граница ≥ прибыли ПОЛНОГО расчёта (costs.rs evaluate_with — та же модель, что у бота) на плотной сетке объёмов
//!    (эталон — перебор) по трём направлениям на записанных данных: биржа → DEX (стакан BingX CAKE-USDT → пул CAKE/USDT
//!    PancakeSwap V2), DEX → биржа (покупка в том же пуле точным выходом → bids BingX), биржа → биржа (asks → bids того
//!    же стакана); верх DEX — функция адаптера (quote/bound.rs rho_raw) на том же пуле;
//! 2) тест-сторож: при любом пороге (USDT и %), при котором граница отсекает, НИ ОДНА точка сетки не проходит пороги;
//!    при котором хоть одна точка проходит — граница НЕ отсекает.
//! 3) решение 390 (t176): то же (1 и 2) при большом запасе k·σ·√T = 0,22 … 8,9 (запас выручка·(1 − e^(−k·σ·√T)) — до
//!    почти всей выручки): κ границы — та же доля e^(−k·σ·√T), что у расчёта (reserve.rs keep), поэтому граница не
//!    отсекает то, что расчёт примет. Для сравнения печатается, сколько точек сетки нарушила бы прежняя
//!    κ = max(0, 1 − k·σ·√T − r_B) при новом расчёте (только печать).
//!
//! Входы вывода и возврата (поправка по фактам, доля, налог перевода, возврат USDT, комиссии в котируемой и в монете,
//! отрицательные входы) — tests/core_sieve_inputs_t162.rs; 18 записанных отказов — tests/core_sieve_recorded_t162.rs.
//! Данные (tests/size_common): стакан BingX CAKE-USDT@depth20, правила пары и сеть BEP20 CAKE у BingX, пул CAKE/USDT V2.
//! Ставки taker в записях нет — 0,1 % (как tests/core_size_budget_early.rs). Затраты эталона, которых нет в записи (газ
//! DEX, перевод с DEX), не вычитаются — прибыль эталона только выше, проверка строже.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{bound, bound_in, costs, dirctx}, tests/sieve_common, tests/size_common.

mod sieve_common;
mod size_common;

use arb::core::Core;
use arb::core::reserve;
use arb::core::types::Side;
use rust_decimal::Decimal;
use sieve_common::{Books, Way, covers, cx, frozen};
use size_common::{Rec, d};

/// Один сценарий: эталон на сетке против границы; сторож по набору порогов. Возвращает: (точек, отсечений).
async fn scenario(core: &Core, rec: &Rec, w: Way, sigma: Decimal, k: Decimal) -> (usize, usize) {
    let c = cx(rec, w, sigma, k);
    let books = Books {
        a: frozen(rec, Side::Buy),
        b: frozen(rec, Side::Sell),
    };
    let (bound, pts) = Box::pin(covers(core, rec, w, &c, &books, None)).await;
    assert!(
        pts.len() > 20,
        "{w:?}: допустимых точек сетки мало: {}",
        pts.len()
    );
    let best_p = pts.iter().map(|p| p.1).max().unwrap();
    let mut cuts = 0;
    let usd = [
        best_p - d("1"),
        best_p - d("0.0001"),
        best_p,
        best_p + d("0.0001"),
        best_p + d("1"),
        d("-1000"),
        d("1000"),
    ];
    for t_usd in usd {
        for t_pct in [d("-100"), d("0"), d("0.1"), d("1"), d("5"), d("50")] {
            let best = bound.best(Some(t_pct)).expect("максимумы границы");
            let cut = best.below_usd(t_usd) || best.below_pct();
            let passes = pts.iter().any(|(_, p, pc)| *p >= t_usd && *pc >= t_pct);
            assert!(
                !(cut && passes),
                "{w:?}: отсечено при пороге {t_usd} / {t_pct}%, а точка сетки проходит"
            );
            cuts += usize::from(cut);
        }
    }
    (pts.len(), cuts)
}

/// Граница ≥ эталона на плотной сетке и сторож — по трём направлениям и двум запасам (σ = 0 и σ записи сценария).
#[tokio::test]
async fn bound_covers_full_calc_on_dense_grid() {
    let dirp = std::env::temp_dir().join(format!("arb_sieve_t162_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = sieve_common::core(&dirp).await;
    let rec = Rec::load("sieve-t162").await;
    let mut total_cuts = 0;
    for w in [Way::CexDex, Way::DexCex, Way::CexCex] {
        for (sigma, k) in [
            (d("0"), d("0")),
            (d("0.0005"), d("1")),
            (d("0.002"), d("-1")),
        ] {
            let (n, cuts) = Box::pin(scenario(&core, &rec, w, sigma, k)).await;
            println!("{w:?} σ {sigma} k {k}: точек {n}, отсечений по порогам {cuts}");
            total_cuts += cuts;
        }
    }
    assert!(
        total_cuts > 0,
        "граница ни разу не отсекла — проверка пуста"
    );
    let _ = std::fs::remove_dir_all(&dirp);
}

/// Решение 390 (t176): граница ≥ эталона и сторож при большом запасе (k·σ·√T от 0,22 до 8,9; T = 20 с, k = 1) по трём
/// направлениям. Печать: точки, где прежняя κ = max(0, 1 − k·σ·√T − r_B) была бы ниже прибыли нового расчёта.
#[tokio::test]
async fn bound_covers_full_calc_large_reserve_t176() {
    let dirp = std::env::temp_dir().join(format!("arb_sieve_t176_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = sieve_common::core(&dirp).await;
    let rec = Rec::load("sieve-t162").await;
    for w in [Way::CexDex, Way::DexCex, Way::CexCex] {
        for sigma in [d("0.05"), d("0.25"), d("1"), d("2")] {
            let c = cx(&rec, w, sigma, d("1"));
            let books = Books {
                a: frozen(&rec, Side::Buy),
                b: frozen(&rec, Side::Sell),
            };
            let (bound, pts) = Box::pin(covers(&core, &rec, w, &c, &books, None)).await;
            let m = c.k * c.sigma * c.transit_s.sqrt().unwrap();
            let keep = reserve::keep(m);
            assert!(keep > Decimal::ZERO && keep < Decimal::ONE, "{m}: {keep}");
            // κ = max(0, keep − r_B): при κ > 0 r_B = keep − κ; при κ = 0 прежняя κ тоже 0.
            if bound.kappa > Decimal::ZERO {
                let r_b = keep - bound.kappa;
                let mut old = bound.clone();
                old.kappa = (Decimal::ONE - m - r_b).max(Decimal::ZERO);
                let broken = pts
                    .iter()
                    .filter(|(q, p, _)| old.u_at(*q).is_some_and(|(u, _)| u < *p))
                    .count();
                println!(
                    "{w:?} k·σ·√T {m}: κ {} (прежняя {}), точек {}, прежняя граница ниже расчёта в {broken}",
                    bound.kappa,
                    old.kappa,
                    pts.len()
                );
            }
            Box::pin(scenario(&core, &rec, w, sigma, d("1"))).await;
        }
    }
    let _ = std::fs::remove_dir_all(&dirp);
}
