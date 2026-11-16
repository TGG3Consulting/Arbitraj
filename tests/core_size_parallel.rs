//! Файл: tests/core_size_parallel.rs
//! Что делает: решения 336–337 (задача t157) на записанных стакане BingX и пуле CAKE/USDT V2 (tests/size_common):
//! - параллельные точки (size.parallel_points > 1: сетка и изломы пачками, уточнение всех пиков в ногу) дают тот же
//!   итог, что подбор по одной: тот же объём, прибыль, границы, число расчётов;
//! - k проб границ (size.bisect_probes > 1) — те же границы и не хуже итог (признак TooLarge / TooSmall монотонен);
//! - ранний выход (Params.early_floor): пол ниже лучшей прибыли — итог тот же, что без пола; пол выше лучшей прибыли
//!   полного подбора — ранний выход и тот же отказ по порогу (сделка не теряется), расчётов меньше.
//!   Кривая теста считает пачку по одной (Curve::batch по умолчанию) — проверяется сам порядок и набор точек.
//!
//! Отдельно — кривая с настоящей одновременной пачкой (join_all, ревью Н6).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::size*, tests/size_common.

mod size_common;

use arb::core::size::{Curve, Fail, Found, Params, Point};
use arb::core::size_search::search;
use async_trait::async_trait;
use futures_util::future::join_all;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// Бюджеты, на которых сравниваются способы подбора (как tests/core_size_optimum.rs).
const BUDGETS: [&str; 7] = ["5", "20", "100", "1000", "5000", "20000", "100000"];

/// Подбор с допуском 1 % на записях. Ошибки — текстом.
async fn run(rec: &Rec, dir: Dir, p: &Params) -> Result<Found<()>, String> {
    let mut c = RecCurve {
        rec,
        dir,
        slip: Decimal::ONE,
    };
    search(&mut c, p).await.map_err(|e| e.to_string())
}

/// Параллельные точки: тот же итог и тот же набор точек, что по одной.
#[tokio::test]
async fn parallel_points_same_result() {
    let rec = Rec::load("parallel").await;
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        for b in BUDGETS {
            let p = params(&rec, dir, d(b));
            let seq = run(&rec, dir, &p).await;
            for par in [2, 4, 8, 16] {
                let mut q = p.clone();
                q.par_points = par;
                let got = run(&rec, dir, &q).await;
                match (&seq, &got) {
                    // Предел size.max_evals исчерпан — порядок точек мог разойтись (шапка size_par.rs): не сравниваем.
                    (Ok(a), Ok(_)) if a.choice.capped => {}
                    (Ok(a), Ok(g)) => {
                        assert_eq!(a.choice.qty, g.choice.qty, "{dir:?} {b} par {par}");
                        assert_eq!(a.choice.profit, g.choice.profit, "{dir:?} {b} par {par}");
                        assert_eq!(a.choice.max_qty, g.choice.max_qty, "{dir:?} {b} par {par}");
                        assert_eq!(a.choice.min_qty, g.choice.min_qty, "{dir:?} {b} par {par}");
                        assert_eq!(a.choice.evals, g.choice.evals, "{dir:?} {b} par {par}");
                        assert!(!g.early);
                    }
                    (Err(a), Err(g)) => assert_eq!(a, g, "{dir:?} {b} par {par}"),
                    _ => panic!(
                        "{dir:?} {b} par {par}: по одной {} против {}",
                        seq.is_ok(),
                        got.is_ok()
                    ),
                }
            }
            if let Ok(a) = &seq {
                println!(
                    "{dir:?} {b}: объём {} прибыль {} расчётов {}",
                    a.choice.qty,
                    a.choice.profit.round_dp(4),
                    a.choice.evals
                );
            }
        }
    }
}

/// k проб границ: те же границы (монотонный признак) и итог не хуже по одной пробе.
#[tokio::test]
async fn bisect_probes_same_bounds() {
    let rec = Rec::load("probes").await;
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        for b in BUDGETS {
            let p = params(&rec, dir, d(b));
            let seq = run(&rec, dir, &p).await;
            for k in [2, 3, 7] {
                let mut q = p.clone();
                q.par_points = 8;
                q.bisect_probes = k;
                let got = run(&rec, dir, &q).await;
                match (&seq, &got) {
                    (Ok(a), Ok(_)) if a.choice.capped => {}
                    (Ok(a), Ok(g)) => {
                        assert_eq!(a.choice.max_qty, g.choice.max_qty, "{dir:?} {b} k {k}");
                        assert_eq!(a.choice.min_qty, g.choice.min_qty, "{dir:?} {b} k {k}");
                        assert!(g.best.invested <= d(b), "{dir:?} {b} k {k}");
                        // Сетка и уточнение — от тех же границ: тот же оптимум (точки границ — лишь добавка в память).
                        assert!(g.choice.profit >= a.choice.profit, "{dir:?} {b} k {k}");
                    }
                    (Err(_), Err(_)) => {}
                    _ => panic!(
                        "{dir:?} {b} k {k}: по одной {} против {}",
                        seq.is_ok(),
                        got.is_ok()
                    ),
                }
            }
        }
    }
}

/// Ранний выход: пол ниже лучшей прибыли — итог прежний; пол выше — выход раньше, отказ по порогу тот же.
#[tokio::test]
async fn early_exit_keeps_decision() {
    let rec = Rec::load("early").await;
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        for b in BUDGETS {
            let p = params(&rec, dir, d(b));
            let Ok(full) = run(&rec, dir, &p).await else {
                continue;
            };
            let best = full.choice.profit;
            // Пол ниже лучшей: раннего выхода нет — тот же итог.
            let mut low = p.clone();
            low.early_floor = Some(best - d("0.000001"));
            let g = run(&rec, dir, &low).await.unwrap();
            if !g.early {
                assert_eq!(g.choice.qty, full.choice.qty, "{dir:?} {b}");
                assert_eq!(g.choice.profit, best, "{dir:?} {b}");
            } else {
                // Выход возможен только если сетка была ниже пола — тогда лучшая полного подбора выше сетки (уточнение).
                assert!(g.choice.profit < best - d("0.000001"), "{dir:?} {b}");
            }
            // Порог выше лучшей полного подбора на 1 USDT, запас 0: сделки не было бы и так — отказ тот же.
            let mut high = p.clone();
            high.early_floor = Some(best + Decimal::ONE);
            let h = run(&rec, dir, &high).await.unwrap();
            assert!(h.early, "{dir:?} {b}: ранний выход ожидался");
            assert!(h.choice.profit <= best, "{dir:?} {b}");
            assert!(h.choice.evals <= full.choice.evals, "{dir:?} {b}");
            println!(
                "{dir:?} {b}: полный подбор {} расчётов, ранний выход {}",
                full.choice.evals, h.choice.evals
            );
        }
    }
}

/// Кривая с настоящей одновременной пачкой (ревью t157 Н6): batch — join_all по точкам, каждая уступает поток перед
/// счётом (как кривая ядра: точки пачки идут вперемешку, а не по одной).
struct JoinCurve<'a> {
    rec: &'a Rec,
    dir: Dir,
}

#[async_trait]
impl Curve for JoinCurve<'_> {
    type Data = ();

    async fn at(&mut self, q: Decimal) -> Result<Point<()>, Fail> {
        RecCurve {
            rec: self.rec,
            dir: self.dir,
            slip: Decimal::ONE,
        }
        .at(q)
        .await
    }

    async fn batch(&mut self, qtys: Vec<Decimal>) -> Vec<Result<Point<()>, Fail>> {
        let (rec, dir) = (self.rec, self.dir);
        join_all(qtys.into_iter().map(|q| async move {
            tokio::task::yield_now().await;
            let mut c = RecCurve {
                rec,
                dir,
                slip: Decimal::ONE,
            };
            c.at(q).await
        }))
        .await
    }
}

/// Параллельные точки с одновременной пачкой: тот же итог, что подбор по одной (cpu.threads_max здесь не нужна —
/// Params собираются тестом; ограничение Н3 действует при чтении настроек, size_par.rs par_settings).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn joined_batch_same_result() {
    let rec = Rec::load("joined").await;
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        for b in BUDGETS {
            let p = params(&rec, dir, d(b));
            let seq = run(&rec, dir, &p).await;
            let mut q = p.clone();
            q.par_points = 8;
            let mut c = JoinCurve { rec: &rec, dir };
            let got = search(&mut c, &q).await.map_err(|e| e.to_string());
            match (&seq, &got) {
                (Ok(a), Ok(_)) if a.choice.capped => {}
                (Ok(a), Ok(g)) => {
                    assert_eq!(a.choice.qty, g.choice.qty, "{dir:?} {b}");
                    assert_eq!(a.choice.profit, g.choice.profit, "{dir:?} {b}");
                    assert_eq!(a.choice.evals, g.choice.evals, "{dir:?} {b}");
                }
                (Err(a), Err(g)) => assert_eq!(a, g, "{dir:?} {b}"),
                _ => panic!(
                    "{dir:?} {b}: по одной {} против {}",
                    seq.is_ok(),
                    got.is_ok()
                ),
            }
        }
    }
}
