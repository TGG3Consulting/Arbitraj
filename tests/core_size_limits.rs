//! Файл: tests/core_size_limits.rs
//! Что делает: пределы подбора объёма на записанных данных CAKE (решение 66): minQty пары (2.672 — строже, чем
//! minNotional / price и минимум вывода) задаёт наименьший объём в обе стороны; объём не меньше резерва пула — NoRoute
//! адаптера DEX → «слишком большой» (граница сверху, а не отказ кандидата); предел size.max_evals; пыль после
//! продажи (dust::below_order); точность вывода withdrawPrecision.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, tests/size_common.

mod size_common;

use arb::core::costs_withdraw::floor_precision;
use arb::core::dust::below_order;
use arb::core::size::{Curve, Fail};
use arb::core::size_search::search;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// minQty из записи — наименьший объём в обе стороны.
#[tokio::test]
async fn min_qty_is_lower_bound() {
    let rec = Rec::load("minqty").await;
    let min = rec.pair.min_qty.unwrap();
    assert!(
        min > rec.pair.min_notional.unwrap() / rec.bids[0].price,
        "в записи minQty строже minNotional"
    );
    for dir in [Dir::DexToBingx, Dir::BingxToDex] {
        let p = params(&rec, dir, d("50"));
        let f = search(
            &mut RecCurve {
                rec: &rec,
                dir,
                slip: Decimal::ONE,
            },
            &p,
        )
        .await
        .unwrap();
        assert_eq!(f.choice.min_qty, min, "{dir:?}");
    }
}

/// Объём больше резерва пула (213 398 CAKE): NoRoute адаптера — TooLarge; огромный бюджет — граница, не отказ.
#[tokio::test]
async fn no_route_is_upper_bound() {
    let rec = Rec::load("noroute").await;
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    match c.at(d("250000")).await {
        Err(Fail::TooLarge(t)) => assert!(t.contains("bsc_dex"), "{t}"),
        other => panic!("ожидался TooLarge от пула: {other:?}"),
    }
    let p = params(&rec, Dir::DexToBingx, d("1000000000"));
    let f = search(&mut c, &p).await.unwrap();
    assert!(f.choice.max_qty < d("213398"));
}

/// Предел расчётов: исчерпан на границах — отказ с причиной; позже — лучший из посчитанных, в бюджете.
#[tokio::test]
async fn max_evals_caps_search() {
    let rec = Rec::load("cap").await;
    let budget = d("20000");
    let mut p = params(&rec, Dir::DexToBingx, budget);
    p.max_evals = 5;
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    match search(&mut c, &p).await {
        Err(Fail::Stop(t)) => assert!(t.contains("size.max_evals"), "{t}"),
        other => panic!("{other:?}"),
    }
    p.max_evals = 60;
    let f = search(&mut c, &p).await.unwrap();
    assert!(f.choice.capped && f.choice.evals <= 60 && f.best.invested <= budget);
}

/// Пыль: меньше шага или minQty — не ордер; больше minQty — ордер.
#[tokio::test]
async fn dust_after_sale() {
    let rec = Rec::load("dust").await;
    let price = rec.bids[0].price;
    assert!(below_order(&rec.pair, d("0.0004"), price));
    assert!(below_order(&rec.pair, d("2"), price));
    assert!(!below_order(&rec.pair, d("3"), price));
}

/// Точность вывода из записи (withdrawPrecision CAKE = 8): сумма вниз до 8 знаков.
#[tokio::test]
async fn withdraw_precision() {
    let rec = Rec::load("prec").await;
    assert_eq!(rec.net.withdraw_precision, Some(8));
    assert_eq!(
        floor_precision(d("1.123456789"), Some(&rec.net)),
        d("1.12345678")
    );
}
