//! Файл: tests/core_size_t159.rs
//! Что делает: решения 342–343 (задача t159) на записанных стакане BingX и пуле CAKE/USDT V2 (tests/size_common):
//! - решение 342 (порог % — ограничение поиска, ошибка №20): порог % — параметр теста между процентом оптимума по
//!   USDT и наибольшим процентом на сетке; прежний подбор даёт объём, не проходящий %, новый — проходящий, и его
//!   прибыль не ниже любой точки сетки, проходящей % (сетка — эталон);
//! - решение 343: концы участков допустимости по minNotional / maxNotional по худшей цене (core/size_scan.rs regions)
//!   на записанных bids — между двумя соседними точками плотной проверки, где допустимость меняется, всегда есть конец
//!   участка (пределы суммы — параметры теста, стакан — записанный); подбор с участками не хуже прежнего.
//!
//! Пороги и пределы суммы — параметры теста (в записях нет кандидата с положительной прибылью — docs/notes/t159-…
//! п. 6), данные — записанные.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::size*, arb::gateway::frozen, tests/size_common.

mod size_common;

use arb::core::size::{Curve, Fail, Point};
use arb::core::size_scan::regions;
use arb::core::size_search::search;
use arb::core::types::{Asset, OrderBook, Side, VenueId};
use arb::gateway::frozen::deepest;
use arb::venues::bingx::book::walk;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// Кривая записей с процентом прибыли от вложенного (биржа → DEX: вложение — проход по asks, как Calc.invested).
struct Pct<'a> {
    inner: RecCurve<'a>,
}

#[async_trait]
impl Curve for Pct<'_> {
    type Data = Decimal;

    async fn at(&mut self, q: Decimal) -> Result<Point<Decimal>, Fail> {
        let p = self.inner.at(q).await?;
        let (_, cost) = walk(&self.inner.rec.asks, q);
        Ok(Point {
            qty: p.qty,
            invested: p.invested,
            arrive: p.arrive,
            profit: p.profit,
            data: p.profit / cost * Decimal::ONE_HUNDRED,
        })
    }

    fn pct(data: &Decimal) -> Option<Decimal> {
        Some(*data)
    }
}

fn curve(rec: &Rec) -> Pct<'_> {
    Pct {
        inner: RecCurve {
            rec,
            dir: Dir::BingxToDex,
            slip: Decimal::ONE,
        },
    }
}

/// Решение 342: наибольшая прибыль в USDT среди объёмов, проходящих порог %.
#[tokio::test]
async fn pct_is_a_constraint() {
    let rec = Rec::load("t159-pct").await;
    let p = params(&rec, Dir::BingxToDex, d("1000"));
    let mut c = curve(&rec);
    let old = search(&mut c, &p).await.unwrap();
    // Сетка — эталон: каждые 5 монет до наибольшего объёма.
    let mut grid = Vec::new();
    let mut q = p.step;
    while q <= old.choice.max_qty {
        if let Ok(pt) = c.at(q).await
            && pt.invested <= p.budget
        {
            grid.push((q, pt.profit, pt.data));
        }
        q += d("5");
    }
    let top = grid.iter().map(|g| g.2).max().unwrap();
    assert!(
        top > old.best.data,
        "запись: процент на оптимуме по USDT ниже наибольшего на сетке"
    );
    let thr = (top + old.best.data) / Decimal::TWO;
    let mut p2 = p.clone();
    p2.ext.min_pct = Some(thr);
    let mut c2 = curve(&rec);
    let new = search(&mut c2, &p2).await.unwrap();
    assert!(old.best.data < thr, "прежний подбор не проходит порог %");
    assert!(
        new.best.data >= thr,
        "новый проходит порог %: {}",
        new.best.data
    );
    for (q, profit, pct) in &grid {
        if *pct >= thr {
            assert!(
                new.best.profit >= *profit,
                "объём {q}: {profit} > {} (новый {})",
                new.best.profit,
                new.best.qty
            );
        }
    }
}

/// Решение 343: концы участков допустимости по minNotional / maxNotional по худшей цене.
#[tokio::test]
async fn regions_bracket_every_change() {
    let rec = Rec::load("t159-regions").await;
    let book = OrderBook {
        venue: VenueId("bingx".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
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
    let ends = regions(&rec.bids, Some(&pair));
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
                ends.iter().any(|e| *e >= s0 && *e <= s),
                "смена допустимости между {s0} и {s} без конца участка"
            );
        }
        prev = Some((s, f));
    }
}

/// Решение 343: подбор с участками допустимости не хуже прежнего (точки — надмножество прежних).
#[tokio::test]
async fn scan_not_worse() {
    let rec = Rec::load("t159-scan").await;
    for b in ["20", "1000", "20000"] {
        let p = params(&rec, Dir::BingxToDex, d(b));
        let mut c = curve(&rec);
        let old = search(&mut c, &p).await.unwrap();
        let mut p2 = p.clone();
        // Продажа — в пул (B — DEX): концов участков по стакану B нет, только точки вне отрезка бисекции.
        p2.ext.scan = true;
        let mut c2 = curve(&rec);
        let new = search(&mut c2, &p2).await.unwrap();
        assert!(
            new.best.profit >= old.best.profit,
            "бюджет {b}: {} < {}",
            new.best.profit,
            old.best.profit
        );
    }
}
