//! Модель контрфакта по плану MUB. Все цены, сроки и результаты относятся к записи и runner.
#[path = "size_common/mub_recording.rs"]
mod mub_recording;
#[path = "size_common/plateau.rs"]
mod plateau;
mod sieve_common;
mod size_common;

use arb::core::costs::{Calc, Live, Source};
use arb::core::costs_quote::floor_step;
use arb::core::costs_withdraw::floor_precision;
use arb::core::size::{Curve, Fail, Params, Point};
use arb::core::size_curve::SourceCurve;
use arb::core::size_memo::Memo;
use arb::core::size_search::search_in;
use mub_recording::{Recording, num};
use plateau::{consider, first_equal, improves};
use rust_decimal::Decimal;
use serde_json::json;
use size_common::d;
use std::time::{Duration, Instant};

/// Запись описывает одну фиксированную ставку покупки в монете. Обе сетки,
/// округление вывода и все строки перевода берём из того же источника,
/// что полный расчёт. Объём ордера из этого метода не допускается к продаже.
async fn forward<S: Source>(rec: &Recording, src: &S, units: u64) -> Option<Decimal> {
    let qty = rec.step * Decimal::from(units);
    let sent = floor_precision(qty - qty * rec.fee, rec.cx.a.info.as_ref());
    let fees = src.withdraw(&rec.cx, sent).await.ok()?;
    let taken: Decimal = fees
        .iter()
        .filter(|c| !c.in_price && arb::core::convert::in_coin(c, &rec.cx.dir.asset.ticker))
        .map(|c| c.amount)
        .sum();
    Some(floor_step(
        (sent - taken).max(Decimal::ZERO),
        rec.cx.pair_b.as_ref(),
    ))
}

fn params(rec: &Recording, budget: Decimal) -> Params {
    let mut p = Params {
        step: rec.step,
        budget,
        guess: budget / num(&rec.raw["plan"], "buy_avg"),
        buy_breaks: vec![],
        sell_breaks: vec![],
        bisect_steps: 12,
        grid_points: 16,
        refine_steps: 30,
        tolerance_pct: Decimal::ONE,
        max_evals: 300,
        par_points: 4,
        bisect_probes: 3,
        early_floor: None,
        reuse: false,
        par_blocked: false,
        ext: Default::default(),
    };
    p.ext.budget_monotone = true;
    p.ext.min_usd = Some(d("0.05"));
    p.ext.min_pct = Some(Decimal::ZERO);
    p
}

fn fields(p: &Point<Calc>) -> serde_json::Value {
    json!({"qty":p.qty,"sell_qty":p.data.sell_qty,"arrive":p.arrive,"spend_max":p.invested,"profit_usdt":p.profit})
}

async fn compare_quota<S: Source>(
    rec: &Recording,
    src: &S,
    edge: u64,
    planned: u64,
    used: u64,
    cached: bool,
) {
    let p = params(rec, d("500"));
    let mut curve = SourceCurve { cx: &rec.cx, src };
    let mut m = Memo::new(&mut curve, p.step, p.budget, p.max_evals)
        .with_gate(p.ext.min_usd, p.ext.min_pct);
    // Подлинные полные расчёты одной ступени. Это контролируемый префикс,
    // 300 исторических ключей сделка не сохраняет.
    for n in planned..planned + used - u64::from(cached) {
        m.at(n).await.unwrap();
    }
    if cached {
        m.at(edge).await.unwrap();
    }
    let old = m.best().unwrap();
    assert_eq!(m.evals(), used as u32);
    let target = old.data.sell_qty;
    let projected = |u| forward(rec, src, u);
    assert!(
        consider(&mut m, &old, target, Instant::now(), projected)
            .await
            .is_none()
    );
    assert_eq!(m.evals(), used as u32);
    let start = Instant::now();
    let candidate = consider(
        &mut m,
        &old,
        target,
        start + Duration::from_secs(5),
        projected,
    )
    .await;
    let took = start.elapsed().as_micros();
    let chosen = candidate.as_ref().unwrap_or(&old);
    let extra = m.evals() - used as u32;
    assert_eq!(extra, u32::from(used < 300 && !cached));
    assert_eq!(
        chosen.qty,
        m.qty(if cached || used < 300 { edge } else { planned })
    );
    println!(
        "VOLUME_PLATEAU {}",
        json!({"case":"MUB_quota_prefix","used":used,
        "limit":300,"cached":cached,"extra_evals":extra,"old":fields(&old),"chosen":fields(chosen),
        "stage_us":took,"expired_extra_evals":0})
    );
}

#[tokio::test]
async fn archived_mub() {
    let mut rec = Recording::load();
    let path = std::env::temp_dir().join(format!("arb-mub-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    let core = sieve_common::core(&path).await;
    let src = Live {
        core: &core,
        src: Some(&rec.books),
    };
    let p = params(&rec, d("500"));
    let planned: u64 = (num(&rec.raw["plan"], "qty") / p.step).try_into().unwrap();
    let target = num(&rec.raw["plan"], "sell_qty");
    let deadline = Instant::now() + Duration::from_secs(5);
    let edge = first_equal(planned, target, deadline, |u| forward(&rec, &src, u))
        .await
        .unwrap();
    assert_eq!(edge, 673274);
    let mut curve = SourceCurve {
        cx: &rec.cx,
        src: &src,
    };
    let old = curve.at(p.step * Decimal::from(planned)).await.unwrap();
    let low = curve.at(p.step * Decimal::from(edge - 1)).await.unwrap();
    let new = curve.at(p.step * Decimal::from(edge)).await.unwrap();
    assert_eq!(
        (low.arrive, low.data.sell_qty),
        (d("0.67199972"), d("0.671"))
    );
    assert_eq!((new.arrive, new.data.sell_qty), (d("0.67200072"), target));
    assert_eq!(
        (old.arrive, old.data.sell_qty),
        (num(&rec.raw["plan"], "arrive"), target)
    );
    assert_eq!(old.invested, num(&rec.raw["plan"], "spend_max"));
    assert!(
        (old.profit - num(&rec.raw["plan"], "profit_usd")).abs() < d("0.000000000000000000000001")
    );
    assert_eq!(old.invested - new.invested, d("0.12797007"));
    assert_eq!(new.profit - old.profit, d("0.12797007"));
    println!(
        "VOLUME_PLATEAU {}",
        json!({"case":"MUB_archive_pair",
        "q_minus_step":fields(&low),"edge":fields(&new),"planned":fields(&old),
        "historical_evals":rec.raw["size"]["evals"],"order_submitted":rec.raw["order_submitted"],
        "model":rec.raw["reconstruction"]})
    );

    // Сначала штатные стадии, включая уточнение порога %. Этот прогон
    // имеет собственный набор ключей и общий лимит 300.
    let mut memo = Memo::new(&mut curve, p.step, p.budget, p.max_evals)
        .with_share(p.par_points, p.reuse)
        .with_gate(p.ext.min_usd, p.ext.min_pct);
    let start = Instant::now();
    let search = search_in(&mut memo, &p).await.unwrap();
    let standard_us = start.elapsed().as_micros();
    let initial = memo.evals();
    let refine = Instant::now();
    let revised = consider(
        &mut memo,
        &search.best,
        search.best.data.sell_qty,
        start + Duration::from_secs(5),
        |u| forward(&rec, &src, u),
    )
    .await;
    let revision_us = refine.elapsed().as_micros();
    let chosen = revised.as_ref().unwrap_or(&search.best);
    assert!(memo.evals() <= p.max_evals);
    // Независимый перебор всех допустимых исходных шагов. Из одного уровня покупки
    // следует точный верх floor(budget/ask). Нижний предел сценария задан по size.min_qty плана.
    let first: u64 = (rec.cx.pair_a.as_ref().unwrap().min_qty.unwrap() / p.step)
        .try_into()
        .unwrap();
    let last: u64 = (p.guess / p.step).floor().try_into().unwrap();
    let mut oracle: Option<Point<Calc>> = None;
    let mut same_sale: Option<Point<Calc>> = None;
    let mut oracle_curve = SourceCurve {
        cx: &rec.cx,
        src: &src,
    };
    let brute_start = Instant::now();
    for i in first..=last {
        let pt = match oracle_curve.at(p.step * Decimal::from(i)).await {
            Ok(pt) => pt,
            Err(Fail::Stop(e)) => panic!("неполный эталон: {e}"),
            Err(_) => continue,
        };
        if pt.invested > p.budget
            || pt.profit < p.ext.min_usd.unwrap()
            || pt.data.profit_pct < p.ext.min_pct.unwrap()
        {
            continue;
        }
        // При точном равенстве меньшее количество на этой плоской книге
        // также имеет меньшее списание. Перебор идёт снизу вверх.
        if pt.data.sell_qty == target && same_sale.as_ref().is_none_or(|b| pt.profit > b.profit) {
            same_sale = Some(pt.clone());
        }
        if oracle.as_ref().is_none_or(|b| pt.profit > b.profit) {
            oracle = Some(pt);
        }
    }
    let oracle = oracle.unwrap();
    assert_eq!(same_sale.unwrap().qty, new.qty);
    assert!(oracle.profit >= chosen.profit && chosen.profit >= search.best.profit);
    println!(
        "VOLUME_PLATEAU {}",
        json!({"case":"MUB_exhaustive_flat_scenario","oracle":fields(&oracle),
        "oracle_first":first,"oracle_last":last,"oracle_calls":last-first+1,"oracle_us":brute_start.elapsed().as_micros(),
        "standard":fields(&search.best),"chosen":fields(chosen),"standard_evals":initial,"after_evals":memo.evals(),
        "standard_us":standard_us,"revision_us":revision_us,"gap_usdt":oracle.profit-chosen.profit})
    );
    for (used, cached) in [(299, false), (300, false), (300, true)] {
        compare_quota(&rec, &src, edge, planned, used, cached).await;
    }
    // Тот же приход при новом минимуме ордера A. Полный расчёт отвергает границу.
    rec.cx.pair_a.as_mut().unwrap().min_notional = Some((old.invested + new.invested) / d("2"));
    let mut at_min = SourceCurve {
        cx: &rec.cx,
        src: &src,
    };
    assert!(matches!(at_min.at(new.qty).await, Err(Fail::TooSmall(_))));
    assert!(at_min.at(old.qty).await.is_ok());
}

#[test]
fn tie_break_uses_exact_spend() {
    let pt = |profit: &str, spend: &str| Point {
        qty: Decimal::ONE,
        invested: d(spend),
        arrive: Decimal::ONE,
        profit: d(profit),
        data: (),
    };
    assert!(improves(&pt("6", "1000"), &pt("6", "2000")));
    assert!(!improves(&pt("6", "2000"), &pt("6", "2000")));
    assert!(!improves(
        &pt("5.999999999999999999", "1000"),
        &pt("6", "2000")
    ));
}

#[tokio::test]
async fn expired_boundary_skips_all_probes() {
    let calls = std::cell::Cell::new(0);
    let edge = first_equal(100, Decimal::ONE, Instant::now(), |_| {
        calls.set(calls.get() + 1);
        std::future::ready(Some(Decimal::ONE))
    })
    .await;
    assert_eq!((edge, calls.get()), (None, 0));
}

#[tokio::test]
async fn changed_transfer_terms() {
    let mut rec = Recording::load();
    rec.step = d("0.000003");
    rec.cx.pair_a.as_mut().unwrap().step_size = rec.step;
    rec.cx.pair_b.as_mut().unwrap().step_size = d("0.0004");
    rec.cx.transfer_tax = Some(d("0.2"));
    let info = rec.cx.a.info.as_mut().unwrap();
    info.withdraw_step = Some(d("0.00001"));
    info.withdraw_fee_pct = Some(d("0.001"));
    let path = std::env::temp_dir().join(format!("arb-mub-terms-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    let core = sieve_common::core(&path).await;
    let src = mub_recording::TransferReplay {
        live: Live {
            core: &core,
            src: Some(&rec.books),
        },
        seen: arb::core::withdraw_seen::Seen {
            asset: rec.cx.dir.asset.clone(),
            from: rec.cx.dir.buy_on.clone(),
            sent: d("1"),
            credited: d("0.995"),
            fee: Some(d("0.002")),
        },
        hold_pct: d("0.1"),
    };
    let mut curve = SourceCurve {
        cx: &rec.cx,
        src: &src,
    };
    let last = 225000u64;
    let plan = curve.at(rec.step * Decimal::from(last)).await.unwrap();
    let target = plan.data.sell_qty;
    let deadline = Instant::now() + Duration::from_secs(5);
    let edge = first_equal(last, target, deadline, |u| forward(&rec, &src, u))
        .await
        .unwrap();
    let mut first = None;
    for u in (last - 500)..=last {
        let actual = curve.at(rec.step * Decimal::from(u)).await.unwrap();
        if actual.data.sell_qty == target && first.is_none() {
            first = Some(u);
        }
    }
    assert_eq!(first, Some(edge));
    let prev = curve.at(rec.step * Decimal::from(edge - 1)).await.unwrap();
    let exact = curve.at(rec.step * Decimal::from(edge)).await.unwrap();
    assert!(prev.data.sell_qty < target && exact.data.sell_qty == target);
    use arb::core::types::CostKind;
    for kind in [
        CostKind::TransferTax,
        CostKind::TransferShortfall,
        CostKind::DepositHold,
    ] {
        assert!(
            exact
                .data
                .costs
                .iter()
                .any(|c| c.kind == kind && c.amount > Decimal::ZERO)
        );
    }
    // Попытка соединить несогласованные тарифные участки: отсутствие цены по
    // другой версии исключает доказательство и не превращается в нулевую комиссию.
    let unavailable = |u| async move {
        if u != last {
            return None;
        }
        Some(target)
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    assert!(
        first_equal(last, target, deadline, unavailable)
            .await
            .is_none()
    );
    println!(
        "VOLUME_PLATEAU {}",
        json!({"case":"synthetic_changed_terms","buy_step":rec.step,
        "sell_step":rec.cx.pair_b.as_ref().unwrap().step_size,"oracle_first":first,
        "q_minus_step":fields(&prev),"edge":fields(&exact),"plan":fields(&plan),"oracle_calls":501,
        "unversioned_tariff_candidate":false})
    );
}
