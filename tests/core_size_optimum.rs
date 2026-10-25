//! Файл: tests/core_size_optimum.rs
//! Что делает: проверка подбора объёма (решение 66, core/size_search.rs) на записанных стакане BingX CAKE и пуле
//! CAKE/USDT PancakeSwap V2 (tests/size_common): вложение никогда не больше бюджета; найденный объём не хуже любого
//! объёма перебора (каждый шаг монеты на малом бюджете, каждая целая монета и уровни стакана на большом) и не хуже
//! прежнего перебора profit::sizes; случай «вложение на бюджете чуть больше бюджета» (живой отказ WFI
//! bsc_dex → bingx: «вложение 20.0000003 больше бюджета 20») — прежний подбор отказывает, новый находит объём.
//! Параметры поиска — предлагаемые значения настроек size.* (docs/DECISIONS.md, решение 66).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::size*, tests/size_common.

mod size_common;

use arb::core::size::{Curve, Fail, Params};
use arb::core::size_curve::cumulative;
use arb::core::size_search::search;
use rust_decimal::Decimal;
use size_common::{Dir, Rec, RecCurve, d};

/// Параметры поиска с предлагаемыми значениями size.*.
fn params(rec: &Rec, dir: Dir, budget: Decimal) -> Params {
    let price = match dir {
        Dir::DexToBingx => rec.pool_price(),
        Dir::BingxToDex => rec.asks[0].price,
    };
    let (buy, sell) = match dir {
        Dir::DexToBingx => (vec![], cumulative(&rec.bids)),
        Dir::BingxToDex => (cumulative(&rec.asks), vec![]),
    };
    Params {
        step: rec.pair.step_size,
        budget,
        guess: budget / price,
        buy_breaks: buy,
        sell_breaks: sell,
        bisect_steps: 32,
        grid_points: 16,
        refine_steps: 30,
        tolerance_pct: d("0.01"),
    }
}

/// Прибыль на объём, если он допустим в бюджете. Ошибок нет.
async fn profit_at(c: &mut RecCurve<'_>, q: Decimal, budget: Decimal) -> Option<Decimal> {
    match c.at(q).await {
        Ok(p) if p.invested <= budget => Some(p.profit),
        _ => None,
    }
}

/// Прежний подбор (profit::sizes до решения 66): бюджет / лучшая цена и уровни стакана, рост до первого спада,
/// вложение больше бюджета — отказ. Возвращает: лучшую прибыль или текст отказа.
async fn old_pick(c: &mut RecCurve<'_>, p: &Params) -> Result<Decimal, String> {
    let max = p.guess;
    let mut sizes = vec![max];
    for cum in p.buy_breaks.iter().chain(&p.sell_breaks) {
        if *cum < max {
            sizes.push(*cum);
        }
    }
    sizes.sort();
    let mut best: Option<Decimal> = None;
    let mut err = String::new();
    for q in sizes {
        match c.at(q).await {
            Ok(x) if x.invested > p.budget => {
                err = format!(
                    "объём {q}: вложение {} больше бюджета {}",
                    x.invested, p.budget
                );
                break;
            }
            Ok(x) => {
                if best.is_some_and(|b| x.profit <= b) {
                    break;
                }
                best = Some(x.profit);
            }
            Err(e) => err = e.to_string(),
        }
    }
    best.ok_or(err)
}

/// Живой случай WFI на записанных данных CAKE: «бюджет / лучшая цена» даёт вложение чуть больше бюджета.
#[tokio::test]
async fn budget_edge_like_wfi() {
    let rec = Rec::load();
    let budget = d("20");
    let p = params(&rec, Dir::DexToBingx, budget);
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
    };
    let first = c.at(p.guess).await.unwrap();
    assert!(
        first.invested > budget,
        "запись должна повторять живой случай: {}",
        first.invested
    );
    let old = old_pick(&mut c, &p).await;
    assert!(old.is_err(), "прежний подбор отказывал: {old:?}");
    let f = search(&mut c, &p).await.unwrap();
    assert!(f.best.invested <= budget);
    assert!(f.choice.max_exact);
    // Наибольший объём точный: следующий шаг уже за бюджетом.
    let next = c.at(f.choice.max_qty + p.step).await.unwrap();
    assert!(next.invested > budget);
    // Перебор каждого шага монеты до бюджета.
    let mut q = p.step;
    while q <= f.choice.max_qty {
        if let Some(v) = profit_at(&mut c, q, budget).await {
            assert!(f.best.profit >= v, "объём {q}: {v} > {}", f.best.profit);
        }
        q += p.step;
    }
    println!("WFI-подобный случай: {:?}", f.choice);
}

/// Большой бюджет: внутренний оптимум (проскальзывание пула против уровней bids), не хуже перебора и прежнего подбора.
#[tokio::test]
async fn interior_optimum_dex_to_bingx() {
    let rec = Rec::load();
    let budget = d("20000");
    let p = params(&rec, Dir::DexToBingx, budget);
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
    };
    let f = search(&mut c, &p).await.unwrap();
    assert!(f.best.invested <= budget);
    // Оптимум внутри отрезка: на наибольшем объёме прибыль меньше.
    assert!(f.choice.qty < f.choice.max_qty && f.choice.at_budget_profit < f.choice.profit);
    let mut grid: Vec<Decimal> = (1..=10000).map(Decimal::from).collect();
    for b in &p.sell_breaks {
        grid.extend([*b - p.step, *b, *b + p.step]);
    }
    let mut brute = Decimal::MIN;
    for q in grid.into_iter().filter(|q| *q > Decimal::ZERO) {
        if let Some(v) = profit_at(&mut c, q, budget).await {
            brute = brute.max(v);
        }
    }
    assert!(
        f.best.profit >= brute,
        "оптимум {} хуже перебора {brute}",
        f.best.profit
    );
    if let Ok(old) = old_pick(&mut c, &p).await {
        assert!(f.best.profit >= old);
    }
    println!(
        "CAKE bsc_dex → bingx, бюджет {budget}: {:?}, перебор {brute}",
        f.choice
    );
}

/// Обратное направление (убыточное): наименьший объём — по минимумам BingX, оптимум не хуже перебора.
#[tokio::test]
async fn minimums_bingx_to_dex() {
    let rec = Rec::load();
    let budget = d("50");
    let p = params(&rec, Dir::BingxToDex, budget);
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::BingxToDex,
    };
    let f = search(&mut c, &p).await.unwrap();
    assert!(f.best.invested <= budget);
    let below = c.at(f.choice.min_qty - p.step).await;
    assert!(matches!(below, Err(Fail::TooSmall(_))), "{below:?}");
    let mut q = p.step;
    while q <= f.choice.max_qty {
        if let Some(v) = profit_at(&mut c, q, budget).await {
            assert!(f.best.profit >= v);
        }
        q += p.step;
    }
    println!("CAKE bingx → bsc_dex, бюджет {budget}: {:?}", f.choice);
}

/// Инвариант: на любом бюджете вложение не больше бюджета (или отказ), в обе стороны.
#[tokio::test]
async fn invested_never_above_budget() {
    let rec = Rec::load();
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        for b in [
            "0.5", "1", "2", "5", "20", "100", "1000", "5000", "20000", "100000",
        ] {
            let budget = d(b);
            let p = params(&rec, dir, budget);
            let mut c = RecCurve { rec: &rec, dir };
            match search(&mut c, &p).await {
                Ok(f) => {
                    assert!(f.best.invested <= budget, "{dir:?} {b}");
                    // Объём кратен шагу монеты (округление вниз до stepSize).
                    assert!(
                        (f.choice.qty % p.step).is_zero(),
                        "{dir:?} {b}: {}",
                        f.choice.qty
                    );
                    println!(
                        "{dir:?} бюджет {b}: объём {} прибыль {} расчётов {}",
                        f.choice.qty,
                        f.choice.profit.round_dp(4),
                        f.choice.evals
                    );
                }
                Err(e) => println!("{dir:?} бюджет {b}: отказ — {e}"),
            }
        }
    }
}
