//! Измерение точности подбора на фиксированной комбинации стакана CAKE и пула V2
//! из разных дат (источники и отсутствующие затраты — size_common/mod.rs).
//! Сравнение оценивает только дискретный поиск на общей кривой, без отправки ордеров.

use std::time::Instant;

use super::{Curve, Decimal, Dir, Fail, Pct, Point, Rec, RecCurve, d, params, regions, search};
use serde_json::json;

type Sample = Point<(Decimal, Decimal)>;

fn curve(rec: &Rec, dir: Dir) -> Pct<'_> {
    Pct {
        inner: RecCurve {
            rec,
            dir,
            slip: Decimal::ONE,
        },
    }
}

fn passes(point: &Sample, min_usd: Decimal, min_pct: Decimal) -> bool {
    point.profit >= min_usd && point.data.0 >= min_pct
}

/// При равной прибыли выбираем меньший объём, как в size_gate::best_pct.
fn keep(best: &mut Option<Sample>, point: &Sample) {
    if best.as_ref().is_none_or(|p| point.profit > p.profit) {
        *best = Some(point.clone());
    }
}

pub(super) async fn measure() {
    let rec = Rec::load("size-replay").await;
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        for budget in [d("20"), d("100"), d("400")] {
            let mut p = params(&rec, dir, budget);
            // Численные параметры по снимку рабочего бота 22 ноября.
            p.bisect_steps = 12;
            p.bisect_probes = 3;
            p.par_points = 4;
            p.tolerance_pct = Decimal::ONE;
            p.ext.no_formula = true;
            p.ext.budget_monotone = matches!(dir, Dir::BingxToDex);
            if matches!(dir, Dir::DexToBingx) {
                p.ext.sell_regions = regions(&rec.bids, Some(&rec.pair));
            }
            // Верхняя оценка независима от результата поиска: лучшая цена ask или спот
            // V2 до комиссии — нижняя граница цены покупки при любом объёме этой кривой.
            let steps: u32 = (p.guess / p.step).ceil().try_into().unwrap();
            assert!(steps <= 1_000_000, "проверьте изменившийся набор данных");
            let thresholds = [(d("0.1"), d("0.00001")), (d("2"), d("1"))];
            let mut passing = vec![None; thresholds.len()];
            let mut best = None;
            let mut oracle = curve(&rec, dir);
            let oracle_start = Instant::now();
            for n in 1..=steps {
                let point = match oracle.at(p.step * Decimal::from(n)).await {
                    Ok(point) if point.invested <= budget => point,
                    Ok(_) | Err(Fail::TooSmall(_) | Fail::TooLarge(_) | Fail::OverBudget(_)) => {
                        continue;
                    }
                    Err(Fail::Stop(why)) => panic!("ошибка эталонной котировки: {why}"),
                };
                keep(&mut best, &point);
                for (i, &(usd, pct)) in thresholds.iter().enumerate() {
                    if passes(&point, usd, pct) {
                        keep(&mut passing[i], &point);
                    }
                }
            }
            let oracle_us = oracle_start.elapsed().as_micros();
            for (i, (usd, pct)) in thresholds.into_iter().enumerate() {
                p.ext.min_usd = Some(usd);
                p.ext.min_pct = Some(pct);
                let start = Instant::now();
                let result = search(&mut curve(&rec, dir), &p).await;
                let search_us = start.elapsed().as_micros();
                let found = result.as_ref().ok();
                let selected = found.map(|f| &f.best);
                let search_pass = selected.is_some_and(|x| passes(x, usd, pct));
                let oracle_pass = passing[i].is_some();
                let reference = passing[i].as_ref().or(best.as_ref());
                let value = |pt: Option<&Sample>| pt.map_or(Decimal::ZERO, |x| x.profit);
                let regret = value(passing[i].as_ref()) - value(selected.filter(|_| search_pass));
                assert!(regret >= Decimal::ZERO);
                if let Some(s) = selected {
                    assert!(s.invested <= budget && s.qty <= p.step * Decimal::from(steps));
                }
                assert!(!search_pass || oracle_pass);
                let row = json!({
                    "direction": format!("{dir:?}"), "budget_usdt": budget,
                    "min_usdt": usd, "min_pct": pct, "search_pass": search_pass,
                    "oracle_pass": oracle_pass, "regret_usdt": regret,
                    "search_qty": selected.map(|x| x.qty), "search_profit": selected.map(|x| x.profit),
                    "oracle_qty": reference.map(|x| x.qty), "oracle_profit": reference.map(|x| x.profit),
                    "search_calls": found.map(|f| f.choice.evals), "oracle_calls": steps,
                    "search_us": search_us, "oracle_us": oracle_us,
                    "search_error": result.as_ref().err().map(Fail::text),
                });
                println!("SIZE_REPLAY {row}");
            }
        }
    }
}
