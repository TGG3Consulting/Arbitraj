//! Парная модель быстрой предпроверки на записанных стакане и пуле. Сигналы, бюджет и окна сценарные.
use arb::core::costs::{Calc, Source};
use arb::core::precheck_fast::{accept, floor_qty};
use arb::core::size::{Curve, Fail, Params};
use arb::core::size_curve::{SourceCurve, cumulative};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::cost_recording::Priced;
use crate::size_common::{Rec, curve::Dir};

/// Стакан и локальный V2 полностью зафиксированы. Повторы по глубине и бюджету соответствуют
/// profit::final_standard; до этого места общая предпроверка и чтение баланса в запись не входят.
async fn fresh(
    curve: &mut SourceCurve<'_, Priced<'_>>,
    p: &Params,
    asked: Decimal,
) -> Result<Calc, String> {
    let eval = |f: Fail| f.to_string();
    let qty = floor_qty(asked, p.step)?;
    let mut c = match curve.at(qty).await {
        Ok(pt) => pt.data,
        Err(Fail::TooLarge(reason)) => {
            let dir = &curve.cx.dir;
            let (a, b) = tokio::join!(
                curve.src.quote(dir, false, qty),
                curve.src.quote(dir, true, qty)
            );
            let fill = a
                .map_err(|f| format!("{reason}; {f}"))?
                .fillable_qty
                .min(b.map_err(|f| format!("{reason}; {f}"))?.fillable_qty);
            let smaller = floor_qty(fill, p.step)?;
            if smaller >= qty {
                return Err(reason);
            }
            curve.at(smaller).await.map_err(eval)?.data
        }
        Err(f) => return Err(eval(f)),
    };
    if c.spend_max > p.budget {
        let smaller = floor_qty(c.qty * p.budget / c.spend_max, p.step)?;
        if smaller >= c.qty {
            return Err(format!("spend_max {} > {}", c.spend_max, p.budget));
        }
        c = curve.at(smaller).await.map_err(eval)?.data;
        if c.spend_max > p.budget {
            return Err(format!(
                "spend_max {} > {} after resize",
                c.spend_max, p.budget
            ));
        }
    }
    Ok(c)
}

#[derive(Serialize)]
struct Economics {
    qty: Decimal,
    sell_qty: Decimal,
    spent_at_average_usdt: Decimal,
    spend_max_usdt: Decimal,
    proceeds_usdt: Decimal,
    costs_outside_price_and_qty_usdt: Decimal,
    profit_usdt: Decimal,
    profit_pct: Decimal,
}

#[derive(Serialize)]
struct Trial {
    source: &'static str,
    asked: Decimal,
    economics: Option<Economics>,
    rejection: Option<String>,
    buy_quote_calls: u32,
    sell_quote_calls: u32,
    ready_us: u128,
}

impl Trial {
    fn admitted(&self) -> Option<&Economics> {
        self.economics.as_ref().filter(|_| self.rejection.is_none())
    }
}

async fn trial(
    curve: &mut SourceCurve<'_, Priced<'_>>,
    p: &Params,
    asked: Decimal,
    source: &'static str,
    start: Instant,
) -> Trial {
    let buy = curve.src.buy_quotes.load(Ordering::Relaxed);
    let sell = curve.src.sell_quotes.load(Ordering::Relaxed);
    let (economics, rejection) = match fresh(curve, p, asked).await {
        Ok(c) => {
            if curve.src.source.dex_a {
                // В этой записи buy exact-out, газ в BNB, нет удержаний CAKE до продажи.
                assert_eq!(c.qty, c.sell_qty);
            }
            let econ = Economics {
                qty: c.qty,
                sell_qty: c.sell_qty,
                spent_at_average_usdt: c.invested,
                spend_max_usdt: c.spend_max,
                proceeds_usdt: c.proceeds,
                costs_outside_price_and_qty_usdt: c.costs_usdt,
                profit_usdt: c.profit_usd,
                profit_pct: c.profit_pct,
            };
            let err = accept(
                c,
                asked,
                p.budget,
                "CAKE",
                (p.ext.min_usd.unwrap(), p.ext.min_pct.unwrap()),
            )
            .err();
            (Some(econ), err)
        }
        Err(err) => (None, Some(err)),
    };
    Trial {
        source,
        asked,
        economics,
        rejection,
        buy_quote_calls: curve.src.buy_quotes.load(Ordering::Relaxed) - buy,
        sell_quote_calls: curve.src.sell_quotes.load(Ordering::Relaxed) - sell,
        ready_us: start.elapsed().as_micros(),
    }
}

/// По одному ближайшему излому ниже и выше. Верхняя граница p.guess доказана для цены
/// неизменяемых asks/V2 до комиссии. Доля h даёт ограничение только на всю доступную глубину.
fn nearby(
    rec: &Rec,
    dir: Dir,
    p: &Params,
    h: Decimal,
    seed: Decimal,
) -> Vec<(Decimal, &'static str)> {
    let (levels, source) = match dir {
        Dir::BingxToDex => (&rec.asks, "ask_depth"),
        Dir::DexToBingx => (&rec.bids, "bid_depth"),
    };
    let depth = cumulative(levels);
    let cap = depth
        .last()
        .copied()
        .filter(|_| matches!(dir, Dir::DexToBingx));
    let mut points = depth
        .into_iter()
        .map(|q| (q, source))
        .chain(cap.map(|q| (q * h, "total_bid_depth_times_h")))
        .map(|(q, src)| ((q / p.step).floor() * p.step, src))
        .filter(|(q, _)| *q > Decimal::ZERO && *q <= p.guess && *q != seed)
        .collect::<Vec<_>>();
    points.sort_by_key(|(q, _)| *q);
    points.dedup_by_key(|(q, _)| *q);
    let lo = points.iter().rev().find(|(q, _)| *q < seed);
    let hi = points.iter().find(|(q, _)| *q > seed);
    let mut result = lo.into_iter().chain(hi).copied().collect::<Vec<_>>();
    result.sort_by_key(|(q, _)| ((*q - seed).abs(), *q));
    result
}

pub async fn observe(
    rec: &Rec,
    dir: Dir,
    p: &Params,
    curve: &mut SourceCurve<'_, Priced<'_>>,
) -> Vec<Value> {
    let mut rows = Vec::new();
    // Заданы независимо от расчёта на проверяемом снимке. Оба — входы эксперимента, не заявки.
    for seed in [Decimal::from(5), Decimal::from(11)] {
        let start = Instant::now();
        let baseline = trial(curve, p, seed, "synthetic_prior_qty", start).await;
        let baseline_us = baseline.ready_us;
        let candidates = nearby(rec, dir, p, curve.cx.h, seed);
        let mut trials = vec![baseline];
        let mut winner = 0;
        for (qty, source) in candidates {
            let mut alt = trial(curve, p, qty, source, start).await;
            // Отклонённая штатная точка всегда сохраняет прежний полный подбор.
            if trials[0].admitted().is_some()
                && let Some(a) = alt.admitted()
                && let Some(b) = trials[winner].admitted()
                && (a.profit_usdt, -a.spend_max_usdt) > (b.profit_usdt, -b.spend_max_usdt)
            {
                winner = trials.len();
            }
            alt.ready_us = start.elapsed().as_micros();
            trials.push(alt);
        }
        let elapsed = start.elapsed().as_micros();
        let windows: Vec<_> = [1_000, 10_000, 50_000u128].into_iter().map(|window| {
            let best = trials.iter().filter(|t| t.ready_us <= window)
                .filter_map(Trial::admitted).max_by_key(|e| (e.profit_usdt, -e.spend_max_usdt));
            json!({"hypothetical_window_us":window, "baseline_ready":baseline_us<=window,
                "extra_points_ready":trials.iter().skip(1).filter(|t| t.ready_us<=window).count(),
                "passing_extra_points_ready":trials.iter().skip(1).filter(|t| t.ready_us<=window && t.admitted().is_some()).count(),
                "best_fast_profit_usdt":best.filter(|_| trials[0].admitted().is_some()).map(|e|e.profit_usdt)})
        }).collect();
        let base = trials[0].admitted();
        let selected = trials[winner].admitted();
        rows.push(json!({
            "seed_qty":seed, "baseline_us":baseline_us, "extra_us":elapsed-baseline_us,
            "total_us":elapsed, "trials":trials, "fallback_required":base.is_none(),
            "selected_fast_qty":selected.map(|e|e.qty),
            "selected_fast_profit_usdt":selected.map(|e|e.profit_usdt),
            "selected_fast_spend_usdt":selected.map(|e|e.spend_max_usdt),
            "increment_usdt":base.zip(selected).map(|(a,b)|b.profit_usdt-a.profit_usdt),
            "windows":windows, "actual_deadline_us":null, "order_sent_at":null,
        }));
    }
    rows
}
