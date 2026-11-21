//! Файл: tests/core_size_t159.rs
//! Что делает: решения 342–345 (задача t159, ревью t159) — подбор объёма ядра (core/size_search.rs):
//! - 342, ровно контрпример ревью (Б1): прибыль 0,03q − 0,0001q² − 1, цена 1, lo = 1, hi = 1000, size.grid_points 10,
//!   порог 0,997 % — проходят только объёмы 95–105 (уже шага сетки 91), ни одна точка обычного поиска порог не
//!   проходит; новый подбор находит наибольшую прибыль среди проходящих (эталон — перебор всех шагов) и запасной
//!   меньший проходящий объём (Н2). Кривая — формула ревью, не рынок: это проверка алгоритма по решению владельца;
//! - 342 на записанных данных (DEX → биржа: пул CAKE V2 → bids BingX): порог между % оптимума по USDT и % наименьшего
//!   объёма; итог проходит порог и лежит у границы «% ≥ порога» (эталон — бисекция по шагам, % на записях убывает с
//!   объёмом — проверено расчётом) не дальше ширины остановки;
//! - 343: участки допустимости на записанных bids — каждая смена допустимости между точками плотной проверки лежит у
//!   конца склеенного участка, концов не больше двух на участок (ревью Б3); maxNotional по худшей цене (параметр теста —
//!   середина сумм уровня 4 по его цене и цене уровня 5) делает допустимое множество несвязным: бисекция берёт верх
//!   первого участка, подбор с участками находит больший объём выше неё с большей прибылью (DEX → биржа);
//! - 344: изломы, которые кривая отдаёт сама (Curve::breaks: накопленные уровни записанных bids — изломы стороны B;
//!   в Params их нет), считаются ПОСЛЕ обычного поиска: при пределе size.max_evals, равном числу точек обычного
//!   поиска, итог тот же (не вытесняют, ревью Б4), при запасе — точек не больше 2 × предел + уточнение пиков;
//! - 342, повторное ревью Н-5: порог задан, оптимум по USDT его проходит — точек и объём ровно как без порога;
//! - 345: мягкий срок уже наступил до подбора: границы им не обрываются (ревью Б2 — раньше Fail::Stop), проходящий
//!   объём из границ — итог без новых точек; проходящего нет — подбор как без срока (ревью Н11).
//!
//! Пороги, пределы суммы и предел изломов — параметры теста, рыночные данные — записанные (tests/size_common).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::size*, arb::gateway::frozen, tests/size_common.

mod size_common;

use std::time::Instant;

use arb::core::costs_quote::floor_step;
use arb::core::size::{Curve, Fail, Params, Point};
use arb::core::size_curve::cumulative;
use arb::core::size_refine::tolerance;
use arb::core::size_scan::{ends, regions};
use arb::core::size_search::search;
use arb::core::types::{Asset, OrderBook, Side, VenueId};
use arb::gateway::frozen::deepest;
use arb::venues::bingx::book::walk;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// Кривая контрпримера ревью: прибыль 0,03q − 0,0001q² − 1, цена 1 (вложение и база % — q). Данные — (%, база).
struct Quad;

#[async_trait]
impl Curve for Quad {
    type Data = (Decimal, Decimal);

    async fn at(&mut self, q: Decimal) -> Result<Point<(Decimal, Decimal)>, Fail> {
        Ok(quad(q))
    }

    fn pct(data: &(Decimal, Decimal)) -> Option<Decimal> {
        Some(data.0)
    }

    fn pct_base(data: &(Decimal, Decimal)) -> Option<Decimal> {
        Some(data.1)
    }
}

/// Точка контрпримера на объём q.
fn quad(q: Decimal) -> Point<(Decimal, Decimal)> {
    let profit = d("0.03") * q - d("0.0001") * q * q - Decimal::ONE;
    Point {
        qty: q,
        invested: q,
        arrive: q,
        profit,
        data: (profit / q * Decimal::ONE_HUNDRED, q),
    }
}

/// Параметры контрпримера: шаг 1, бюджет 1000 (lo = 1, hi = 1000), сетка 10, ширина остановки — шаг.
fn quad_params() -> Params {
    Params {
        step: Decimal::ONE,
        budget: d("1000"),
        guess: d("1000"),
        buy_breaks: vec![],
        sell_breaks: vec![],
        bisect_steps: 32,
        grid_points: 10,
        refine_steps: 30,
        tolerance_pct: d("0.1"),
        max_evals: 300,
        par_points: 1,
        bisect_probes: 1,
        early_floor: None,
        reuse: false,
        par_blocked: false,
        ext: Default::default(),
    }
}

/// Б1: проходящий участок уже шага сетки, ни одна точка обычного поиска его не задевает.
#[tokio::test]
async fn pct_band_narrower_than_grid() {
    let p = quad_params();
    let thr = d("0.997");
    let old = search(&mut Quad, &p).await.unwrap();
    assert_eq!(
        (old.choice.min_qty, old.choice.max_qty),
        (d("1"), d("1000"))
    );
    assert!(old.best.data.0 < thr, "оптимум по USDT не проходит порог %");
    // Эталон — перебор всех шагов.
    let (mut want, mut top) = (None, Decimal::MIN);
    for n in 1..=1000u32 {
        let pt = quad(Decimal::from(n));
        if pt.data.0 >= thr && pt.profit > top {
            (want, top) = (Some(pt.qty), pt.profit);
        }
    }
    let mut p2 = p.clone();
    p2.ext.min_pct = Some(thr);
    p2.ext.min_usd = Some(d("0.5"));
    let new = search(&mut Quad, &p2).await.unwrap();
    assert!(new.pct_moved);
    assert!(
        new.best.data.0 >= thr,
        "итог проходит порог %: {}",
        new.best.data.0
    );
    assert_eq!(
        Some(new.best.qty),
        want,
        "наибольшая прибыль среди проходящих"
    );
    let fb = new.fallback.expect("запасной меньший проходящий объём");
    assert!(fb.qty < new.best.qty && fb.data.0 >= thr && fb.profit >= d("0.5"));
    assert!(new.choice.evals <= p.max_evals);
}

/// Кривая записей с процентом прибыли от вложенного (DEX → биржа: база — USDT покупки в пуле = выручка − прибыль).
struct Pct<'a> {
    inner: RecCurve<'a>,
}

#[async_trait]
impl Curve for Pct<'_> {
    type Data = (Decimal, Decimal);

    async fn at(&mut self, q: Decimal) -> Result<Point<(Decimal, Decimal)>, Fail> {
        let p = self.inner.at(q).await?;
        let rec = self.inner.rec;
        let (_, proceeds) = walk(&rec.bids, floor_step(q, Some(&rec.pair)));
        let base = proceeds - p.profit;
        Ok(Point {
            qty: p.qty,
            invested: p.invested,
            arrive: p.arrive,
            profit: p.profit,
            data: (p.profit / base * Decimal::ONE_HUNDRED, base),
        })
    }

    fn pct(data: &(Decimal, Decimal)) -> Option<Decimal> {
        Some(data.0)
    }

    fn pct_base(data: &(Decimal, Decimal)) -> Option<Decimal> {
        Some(data.1)
    }
}

fn pct_curve(rec: &Rec) -> Pct<'_> {
    Pct {
        inner: RecCurve {
            rec,
            dir: Dir::DexToBingx,
            slip: Decimal::ONE,
        },
    }
}

/// 342 на записанных данных: итог у границы «% ≥ порога» со стороны оптимума по USDT.
#[tokio::test]
async fn pct_edge_on_records() {
    let rec = Rec::load("t159-pct").await;
    let p = params(&rec, Dir::DexToBingx, d("20000"));
    let old = search(&mut pct_curve(&rec), &p).await.unwrap();
    let mut c = pct_curve(&rec);
    let low = c.at(old.choice.min_qty).await.unwrap();
    let thr = (old.best.data.0 + low.data.0) / Decimal::TWO;
    assert!(old.best.data.0 < thr && low.data.0 >= thr);
    // Эталон: граница «% ≥ порога» бисекцией по шагам между наименьшим объёмом и оптимумом по USDT.
    let n = |q: Decimal| (q / p.step).floor();
    let (mut a, mut b) = (n(old.choice.min_qty), n(old.best.qty));
    while b - a > Decimal::ONE {
        let mid = ((a + b) / Decimal::TWO).floor();
        match c.at(mid * p.step).await {
            Ok(pt) if pt.data.0 >= thr => a = mid,
            _ => b = mid,
        }
    }
    let mut p2 = p.clone();
    p2.ext.min_pct = Some(thr);
    let new = search(&mut pct_curve(&rec), &p2).await.unwrap();
    assert!(
        new.pct_moved && new.best.data.0 >= thr,
        "{}",
        new.best.data.0
    );
    let top = (old.choice.max_qty / p.step).floor().try_into().unwrap();
    let tol = Decimal::from(tolerance(top, p.tolerance_pct));
    let got = n(new.best.qty);
    assert!(
        got <= a && got + tol >= a,
        "итог {got}, граница {a}, ширина {tol}"
    );
    let fb = new.fallback.expect("запасной меньший проходящий объём");
    assert!(fb.qty < new.best.qty && fb.data.0 >= thr);
}

/// Повторное ревью t159 Н-5: порог % задан, а оптимум по USDT его проходит — точки и объём ровно как без порога.
#[tokio::test]
async fn pct_passing_optimum_costs_nothing() {
    let rec = Rec::load("t159-pct-free").await;
    let p = params(&rec, Dir::DexToBingx, d("20000"));
    let old = search(&mut pct_curve(&rec), &p).await.unwrap();
    let mut p2 = p.clone();
    // Порог — ровно процент оптимума по USDT (проходит: «не ниже»).
    p2.ext.min_pct = Some(old.best.data.0);
    let new = search(&mut pct_curve(&rec), &p2).await.unwrap();
    assert!(!new.pct_moved && new.fallback.is_none());
    assert_eq!(
        (new.best.qty, new.choice.evals),
        (old.best.qty, old.choice.evals)
    );
}

/// 343: концы склеенных участков допустимости по minNotional / maxNotional по худшей цене (Б3: по участкам).
#[tokio::test]
async fn regions_bracket_every_change() {
    let rec = Rec::load("t159-regions").await;
    let book = OrderBook {
        venue: VenueId("bingx".into()),
        asset: Asset {
            chain: None,
            ticker: "CAKE".into(),
            contract: None,
        },
        bids: rec.bids.clone(),
        asks: vec![],
        exchange_time: None,
        received_at: Utc::now(),
    };
    let depth: Decimal = rec.bids.iter().map(|l| l.qty).sum();
    // Пределы суммы — параметры теста: minNotional — сумма второго уровня по цене последнего (участки рвутся),
    // maxNotional — половина всей суммы стакана по лучшей цене.
    let last = rec.bids.last().unwrap().price;
    let mut pair = rec.pair.clone();
    pair.min_notional = Some(rec.bids[0].qty.max(rec.bids[1].qty) * last);
    pair.max_notional = Some(depth * rec.bids[0].price / Decimal::TWO);
    let runs = regions(&rec.bids, Some(&pair));
    let e = ends(&runs);
    assert!(!runs.is_empty() && e.len() <= 2 * runs.len());
    let ok = |s: Decimal| match deepest(&book, Side::Sell, s) {
        Some(w) => s * w >= pair.min_notional.unwrap() && s * w <= pair.max_notional.unwrap(),
        None => false,
    };
    let n = 4000u32;
    let mut prev: Option<(Decimal, bool)> = None;
    for i in 1..=n {
        let s = depth * Decimal::from(i) / Decimal::from(n);
        let f = ok(s);
        if let Some((s0, f0)) = prev
            && f0 != f
        {
            assert!(
                e.iter().any(|x| *x >= s0 && *x <= s),
                "смена допустимости между {s0} и {s} без конца участка"
            );
        }
        prev = Some((s, f));
    }
}

/// 343: несвязное допустимое множество по maxNotional продажи по худшей цене — объём выше бисекции.
#[tokio::test]
async fn regions_find_volume_above_bisection() {
    let mut rec = Rec::load("t159-above").await;
    // maxNotional — параметр теста: между суммами конца уровня 4 по его цене и по цене уровня 5.
    let cum = cumulative(&rec.bids);
    let max_n = cum[4] * (rec.bids[4].price + rec.bids[5].price) / Decimal::TWO;
    rec.pair.max_notional = Some(max_n);
    let p = params(&rec, Dir::DexToBingx, d("20000"));
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    let old = search(&mut c, &p).await.unwrap();
    let mut p2 = p.clone();
    p2.ext.sell_regions = regions(&rec.bids, Some(&rec.pair));
    p2.ext.no_formula = true;
    let new = search(&mut c, &p2).await.unwrap();
    assert!(
        new.best.qty > old.choice.max_qty && new.best.profit > old.best.profit,
        "новый {} ({}) — бисекция до {} ({})",
        new.best.qty,
        new.best.profit,
        old.choice.max_qty,
        old.best.profit
    );
    assert!(new.best.qty * rec.bids[5].price <= max_n);
}

/// Кривая записей, которая сама отдаёт изломы стороны B (накопленные уровни записанных bids) — как DEX отдаёт изломы
/// пулов маршрута (решение 344).
struct Br<'a> {
    inner: RecCurve<'a>,
}

#[async_trait]
impl Curve for Br<'_> {
    type Data = ();

    async fn at(&mut self, q: Decimal) -> Result<Point<()>, Fail> {
        self.inner.at(q).await
    }

    async fn breaks(
        &mut self,
        _data: &(),
        _max_qty: Decimal,
        _limit: u32,
    ) -> (Vec<Decimal>, Vec<Decimal>) {
        (Vec::new(), cumulative(&self.inner.rec.bids))
    }
}

fn br(rec: &Rec) -> Br<'_> {
    Br {
        inner: RecCurve {
            rec,
            dir: Dir::DexToBingx,
            slip: Decimal::ONE,
        },
    }
}

/// 344: изломы — после обычного поиска и не вытесняют его.
#[tokio::test]
async fn breaks_after_base_search() {
    let rec = Rec::load("t159-breaks").await;
    let mut p = params(&rec, Dir::DexToBingx, d("20000"));
    p.sell_breaks = vec![];
    let old = search(&mut br(&rec), &p).await.unwrap();
    // Предел — ровно точки обычного поиска: итог тот же.
    let mut tight = p.clone();
    tight.max_evals = old.choice.evals;
    tight.ext.breaks_max = Some(8);
    let same = search(&mut br(&rec), &tight).await.unwrap();
    assert_eq!(
        (same.best.qty, same.choice.evals),
        (old.best.qty, old.choice.evals)
    );
    // С запасом: точек не больше 2 × предел + уточнение пиков на изломах, итог не хуже.
    let mut wide = p.clone();
    let limit = 4u32;
    wide.ext.breaks_max = Some(limit);
    let new = search(&mut br(&rec), &wide).await.unwrap();
    let extra = new.choice.evals - old.choice.evals;
    assert!(
        extra <= 2 * limit + limit * (2 * p.refine_steps + 3),
        "{extra}"
    );
    assert!(new.best.profit >= old.best.profit && new.choice.evals <= p.max_evals);
}

/// 345: срок наступил до подбора — границы не обрываются; проходящий есть — итог без новых точек.
#[tokio::test]
async fn soft_deadline_keeps_bounds_and_passing() {
    let rec = Rec::load("t159-late").await;
    let p = params(&rec, Dir::DexToBingx, d("20000"));
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    let old = search(&mut c, &p).await.unwrap();
    let low = c.at(old.choice.min_qty).await.unwrap();
    // Порог USDT — прибыль наименьшего объёма (границы его считают): проходящий есть сразу после границ.
    let mut p2 = p.clone();
    p2.ext.min_usd = Some(low.profit);
    p2.ext.deadline = Some(Instant::now());
    let late = search(&mut c, &p2).await.expect("срок не обрывает границы");
    assert!(late.late && late.best.profit >= low.profit);
    assert!(late.choice.evals < old.choice.evals);
    // Порог выше любой посчитанной прибыли — проходящего нет: подбор как без срока.
    let mut p3 = p.clone();
    p3.ext.min_usd = Some(old.best.profit + Decimal::ONE);
    p3.ext.deadline = Some(Instant::now());
    let full = search(&mut c, &p3).await.unwrap();
    assert!(!full.late);
    assert_eq!(
        (full.best.qty, full.choice.evals),
        (old.best.qty, old.choice.evals)
    );
}
