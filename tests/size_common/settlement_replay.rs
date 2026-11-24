//! Контроль единиц на тех же записях CAKE, что у core_price_reserve. Значения стакана
//! выражаются в другой единице; денежная стоимость каждого уровня сохранена.

use super::*;
use arb::core::settlement_unit::UnitRatio;

fn set_book_unit(book: &mut OrderBook, units: Decimal) {
    for level in book.asks.iter_mut().chain(&mut book.bids) {
        level.qty *= units;
        level.price /= units;
    }
}

fn set_pair_unit(cx: &mut DirCtx, buy: bool, units: Decimal) {
    let pair = if buy { &mut cx.pair_a } else { &mut cx.pair_b };
    let pair = pair.as_mut().unwrap();
    pair.step_size *= units;
    pair.min_qty = pair.min_qty.map(|q| q * units);
    pair.max_qty = pair.max_qty.map(|q| q * units);
    let network = if buy {
        cx.a.info.as_mut()
    } else {
        cx.b.info.as_mut()
    }
    .unwrap();
    for quantity in [
        &mut network.withdraw_min,
        &mut network.withdraw_max,
        &mut network.deposit_min,
        &mut network.withdraw_fee,
        &mut network.withdraw_step,
    ]
    .into_iter()
    .flatten()
    {
        *quantity *= units;
    }
    cx.units_b_per_a = UnitRatio {
        buy_per_raw: if buy { units } else { Decimal::ONE },
        sell_per_raw: if buy { Decimal::ONE } else { units },
        raw_decimals: Some(18),
    };
}

fn same_cash_flow(a: &Calc, b: &Calc) {
    // rust_decimal округляет средние цены и промежуточную цену на границе своей 96-битной точности.
    // Обе записи задают одно и то же количество минимальных единиц USDT (18 decimals).
    for (x, y) in [
        (a.invested, b.invested),
        (a.proceeds, b.proceeds),
        (a.profit_usd, b.profit_usd),
    ] {
        assert_eq!(x.round_dp(18), y.round_dp(18));
    }
    assert_eq!(
        a.profit_usd.is_sign_positive(),
        b.profit_usd.is_sign_positive()
    );
}

#[tokio::test]
async fn pool_to_spot_remains_profitable_with_scaled_sell_book() {
    let rec = Rec::load("units-pool-spot").await;
    let dir = dir("bsc_dex", "bingx");
    let qty = d("10");
    let baseline_ctx = cx(&rec, dir.clone(), Decimal::ZERO, Some(d("0.5")));
    let baseline = costs::evaluate_with(&baseline_ctx, qty, &PoolToBook::new(&rec))
        .await
        .unwrap();
    assert!(baseline.profit_usd > Decimal::ZERO);
    for scale in [d("8"), d("0.125")] {
        let mut src = PoolToBook::new(&rec);
        set_book_unit(&mut src.sell.book, scale);
        let mut context = cx(&rec, dir.clone(), Decimal::ZERO, Some(d("0.5")));
        set_pair_unit(&mut context, false, scale);
        let calculated = costs::evaluate_with(&context, qty, &src).await.unwrap();
        assert_eq!(calculated.sell_qty, baseline.sell_qty * scale);
        same_cash_flow(&baseline, &calculated);
    }
}

#[tokio::test]
async fn spot_to_pool_sends_the_same_raw_quantity() {
    let rec = Rec::load("units-spot-pool").await;
    let dir = dir("bingx", "bsc_dex");
    let qty = d("10");
    let src = replay(&rec, &dir, false);
    let baseline_ctx = cx(&rec, dir.clone(), Decimal::ZERO, Some(d("0.5")));
    let baseline = costs::evaluate_with(&baseline_ctx, qty, &src)
        .await
        .unwrap();
    let plan =
        Plan::parse(&serde_json::to_value(size_common::calc::calc(&rec, qty)).unwrap()).unwrap();
    for scale in [d("8"), d("0.125")] {
        let mut buy = book(&rec, "bingx");
        set_book_unit(&mut buy, scale);
        let src = Replay::new(&dir, &plan, buy, None, ["USDT", "USDT"]).unwrap();
        let mut context = cx(&rec, dir.clone(), Decimal::ZERO, Some(d("0.5")));
        set_pair_unit(&mut context, true, scale);
        let calculated = costs::evaluate_with(&context, qty * scale, &src)
            .await
            .unwrap();
        assert_eq!(calculated.sell_qty, baseline.sell_qty);
        same_cash_flow(&baseline, &calculated);
    }
}
