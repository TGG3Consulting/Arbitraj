//! Сравнение подбора и перебора по шагу CAKE на одном полном расчёте затрат.
//! Рыночные записи, параметры сценария и границы переноса на торговлю описаны в docs/notes/size-cost-replay.md.

#[path = "size_common/cost_recording.rs"]
mod cost_recording;
mod sieve_common;
mod size_common;

use arb::core::costs::{Calc, Live};
use arb::core::frozen::Frozen;
use arb::core::size::{Curve, Fail, Point};
use arb::core::size_curve::SourceCurve;
use arb::core::size_scan::regions;
use arb::core::size_search::search;
use arb::core::types::Side;
use chrono::Utc;
use cost_recording::{Priced, recording};
use rust_decimal::Decimal;
use serde_json::json;
use sieve_common::{Src, Way};
use size_common::{Rec, curve::Dir, d, params};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

fn passes(point: &Point<Calc>, usd: Decimal, pct: Decimal) -> bool {
    point.profit >= usd && point.data.profit_pct >= pct
}

/// Параметры поиска из снимка настроек 23.11. Нижняя граница цены для перебора — из независимой записи
/// стакана либо из соотношения резервов до комиссии V2, даже если подбор остановился раньше.
async fn compare(
    rec: &Rec,
    curve: &mut SourceCurve<'_, Priced<'_>>,
    dir: Dir,
    budget: Decimal,
    case: &str,
) {
    let mut p = params(rec, dir, budget);
    p.bisect_steps = 12;
    p.bisect_probes = 3;
    p.par_points = 4;
    p.tolerance_pct = Decimal::ONE;
    p.ext.no_formula = true;
    p.ext.budget_monotone = matches!(dir, Dir::BingxToDex);
    if matches!(dir, Dir::DexToBingx) {
        p.ext.sell_regions = regions(&rec.bids, Some(&rec.pair));
    }
    let usd = match dir {
        Dir::DexToBingx => d("0.05"),
        Dir::BingxToDex => d("0.001"),
    };
    let pct = d("0.00001");
    p.ext.min_usd = Some(usd);
    p.ext.min_pct = Some(pct);
    let steps: u32 = (p.guess / p.step).ceil().try_into().unwrap();
    assert!(steps <= 1_000_000);
    let mut best: Option<Point<Calc>> = None;
    let mut passing: Option<Point<Calc>> = None;
    let mut feasible = 0u32;
    let mut rejected = [0u32; 3];
    let started = Instant::now();
    for n in 1..=steps {
        let pt = match curve.at(p.step * Decimal::from(n)).await {
            Ok(pt) if pt.invested <= budget => pt,
            Err(Fail::TooSmall(_)) => {
                rejected[0] += 1;
                continue;
            }
            Err(Fail::TooLarge(_)) => {
                rejected[1] += 1;
                continue;
            }
            Ok(_) | Err(Fail::OverBudget(_)) => {
                rejected[2] += 1;
                continue;
            }
            Err(Fail::Stop(why)) => panic!("источник полного расчёта недоступен: {why}"),
        };
        feasible += 1;
        if passes(&pt, usd, pct) && passing.as_ref().is_none_or(|b| pt.profit > b.profit) {
            passing = Some(pt.clone());
        }
        if best.as_ref().is_none_or(|b| pt.profit > b.profit) {
            best = Some(pt);
        }
    }
    let oracle_us = started.elapsed().as_micros();
    // Время поиска после общего перебора: отдельно от стоимости чтения записи и прогрева пула.
    for ceiling in [p.max_evals, 24] {
        p.max_evals = ceiling;
        curve.src.buy_quotes.store(0, Ordering::Relaxed);
        let started = Instant::now();
        let result = search(curve, &p).await;
        let search_us = started.elapsed().as_micros();
        let buy_quotes = curve.src.buy_quotes.load(Ordering::Relaxed);
        let chosen = result.as_ref().ok().map(|r| &r.best);
        let search_pass = chosen.is_some_and(|pt| passes(pt, usd, pct));
        let oracle_pass = passing.is_some();
        assert!(feasible > 0, "весь диапазон недопустим");
        if let Some(pt) = chosen {
            let verified = curve.at(pt.qty).await.unwrap();
            assert_eq!(
                (verified.profit, verified.invested),
                (pt.profit, pt.invested)
            );
            assert!(
                pt.invested <= budget
                    && pt.qty % p.step == Decimal::ZERO
                    && pt.qty <= p.step * Decimal::from(steps)
            );
        }
        let oracle = passing.as_ref().or(best.as_ref());
        let trade_delta = passing.as_ref().map_or(Decimal::ZERO, |pt| pt.profit)
            - chosen
                .filter(|_| search_pass)
                .map_or(Decimal::ZERO, |pt| pt.profit);
        let raw_gap = best.as_ref().zip(chosen).map(|(a, b)| a.profit - b.profit);
        assert!(trade_delta >= Decimal::ZERO && raw_gap.is_none_or(|n| n >= Decimal::ZERO));
        assert!(!search_pass || oracle_pass);
        let row = json!({
            "case":case,"max_evals":ceiling,"timing_basis":"after_oracle","direction":format!("{dir:?}"),"budget_usdt":budget,
            "min_usdt":usd,"min_pct":pct,"search_pass":search_pass,"oracle_pass":oracle_pass,
            "false_accept":search_pass&&!oracle_pass,"false_reject":!search_pass&&oracle_pass,
            "regret_usdt":trade_delta,"unconstrained_profit_gap_usdt":raw_gap,
            "search_error":result.as_ref().err().map(Fail::text),
            "search_qty":chosen.map(|pt|pt.qty),"search_profit":chosen.map(|pt|pt.profit),
            "search_spend_max":chosen.map(|pt|pt.invested),"oracle_qty":oracle.map(|pt|pt.qty),
            "oracle_profit":oracle.map(|pt|pt.profit),"oracle_spend_max":oracle.map(|pt|pt.invested),
            "search_calls":buy_quotes, "search_evals":result.as_ref().ok().map(|r|r.choice.evals),
            "search_capped":result.as_ref().ok().map(|r|r.choice.capped),
            "oracle_calls":steps,"oracle_admissible":feasible,"below_min":rejected[0],
            "above_limit":rejected[1],"over_budget":rejected[2],"oracle_us":oracle_us,"search_us":search_us
        });
        println!("SIZE_REPLAY {row}");
    }
}

#[tokio::test]
#[ignore = "полный перебор запускает workflow Recorded size replay"]
async fn full_cost_volume_replay() {
    let (rec, gas) = recording().await;
    let path = std::env::temp_dir().join(format!("arb-cost-replay-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    let core = sieve_common::core(&path).await;
    let archived = Rec::load("archived-cost-volume").await;
    for (case, rec) in [
        ("aligned_full_costs", &rec),
        ("aligned_percentage_sensitivity", &rec),
        ("mixed_dates_full_cost_control", &archived),
    ] {
        let books = Frozen {
            a: Arc::new(sieve_common::frozen(rec, Side::Buy)),
            b: Arc::new(sieve_common::frozen(rec, Side::Sell)),
            at: Utc::now(),
            decided: None,
        };
        for (dir, w) in [
            (Dir::BingxToDex, Way::CexDex),
            (Dir::DexToBingx, Way::DexCex),
        ] {
            let k = if w == Way::CexDex { d("2") } else { d("3.6") };
            let mut cx = sieve_common::cx(rec, w, d("0.0000283677823166848681016142"), k);
            cx.transit_s = d("164.55740");
            cx.keep = arb::core::reserve::keep_of(cx.k, cx.sigma, cx.transit_s);
            cx.return_per_trade = true;
            cx.usdt_b = Some(sieve_common::usdt_net());
            if case == "aligned_percentage_sensitivity" {
                // Параметр чувствительности, наблюдавшийся для POSI BEP20 (публичный справочник KuCoin
                // tests/data/cex_samples_2026_11_04/kucoin_currencies_v3.json). CAKE ему не приписываем.
                let info = if w == Way::CexDex {
                    cx.a.info.as_mut()
                } else {
                    cx.usdt_b.as_mut()
                };
                info.unwrap().withdraw_fee_pct = Some(d("0.01"));
            }
            let source = Priced {
                buy_quotes: Default::default(),
                source: Src {
                    live: Live {
                        core: &core,
                        src: Some(&books),
                    },
                    rec,
                    dex_a: w == Way::DexCex,
                    dex_b: w == Way::CexDex,
                },
                gas: &gas,
                dex_slippage_pct: d("0.5"),
            };
            let mut curve = SourceCurve {
                cx: &cx,
                src: &source,
            };
            let budgets: &[u32] = if case == "aligned_full_costs" {
                &[20, 40]
            } else {
                &[20]
            };
            for &budget in budgets {
                compare(rec, &mut curve, dir, Decimal::from(budget), case).await;
            }
        }
    }
}
