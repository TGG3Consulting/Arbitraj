//! Файл: tests/core_size_min_ceiling.rs
//! Что делает: решение 402 (t187) на записанных данных CAKE (tests/size_common: стакан BingX, правила пары с minQty
//! 2.672, сеть BEP20, пул CAKE/USDT V2) — граница объёма у минимума площадки A:
//! - «помещается»: настоящий верх (бюджет) чуть выше minQty — наименьший объём ровно minQty (первый шаг не ниже
//!   минимума, на шаг ниже — «меньше минимума»), сделка возможна;
//! - «не помещается»: на minQty объём уже «слишком большой» (вложение больше бюджета) — отказ вида TooSmall, текст
//!   называет и минимум, и настоящую причину на следующем шаге (граница точная);
//! - бисекции не хватило size.bisect_steps — текст называет ближайшую посчитанную точку и не утверждает соседний шаг.
//!
//! Тот же класс, что живые отказы 17.11 (DIN, MAV, BNT, RWS, CREPE): минимум A проверяется раньше верхнего ограничения
//! (core/costs.rs; кривая теста — тот же порядок), верх бисекции встаёт на шаг ниже минимума.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::size*, tests/size_common.

mod size_common;

use arb::core::size::{Curve, Fail};
use arb::core::size_search::search;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// Кривая BingX → DEX по записям с допуском 1 %.
fn curve(rec: &Rec) -> RecCurve<'_> {
    RecCurve {
        rec,
        dir: Dir::BingxToDex,
        slip: Decimal::ONE,
    }
}

/// Бюджет чуть выше суммы minQty по худшей цене: наименьший объём — ровно minQty, сделка возможна.
#[tokio::test]
async fn min_fits_under_ceiling() {
    let rec = Rec::load("min-ceiling-fits").await;
    let min = rec.pair.min_qty.unwrap();
    let p = params(&rec, Dir::BingxToDex, d("6"));
    let mut c = curve(&rec);
    let at_min = c.at(min).await.unwrap();
    assert!(at_min.invested <= p.budget, "{}", at_min.invested);
    match c.at(min - p.step).await {
        Err(Fail::TooSmall(t)) => assert!(t.contains("minQty"), "{t}"),
        other => panic!("ожидался TooSmall на шаг ниже minQty: {other:?}"),
    }
    let f = search(&mut c, &p).await.unwrap();
    assert_eq!(f.choice.min_qty, min);
    assert!(f.choice.max_qty >= min && f.best.invested <= p.budget);
}

/// Бюджет меньше суммы minQty: отказ «меньше минимума» с настоящей причиной на следующем шаге.
#[tokio::test]
async fn min_over_ceiling_names_real_cause() {
    let rec = Rec::load("min-ceiling-over").await;
    let min = rec.pair.min_qty.unwrap();
    let p = params(&rec, Dir::BingxToDex, d("5"));
    let mut c = curve(&rec);
    assert!(c.at(min).await.unwrap().invested > p.budget);
    match search(&mut c, &p).await {
        Err(Fail::TooSmall(t)) => {
            let below = (min - p.step).normalize();
            assert!(
                t.contains(&format!("даже наибольший объём {below} меньше минимума")),
                "{t}"
            );
            assert!(t.contains("minQty"), "{t}");
            let next = format!("объём {} (следующий шаг) уже больше", min.normalize());
            assert!(t.contains(&next), "{t}");
            assert!(t.contains("больше бюджета 5"), "{t}");
        }
        other => panic!("ожидался TooSmall: {other:?}"),
    }
}

/// Шагов бисекции не хватило: дописка — ближайшая посчитанная точка, без утверждения о соседнем шаге.
#[tokio::test]
async fn unconverged_bound_says_nearest_counted() {
    let rec = Rec::load("min-ceiling-steps").await;
    let mut p = params(&rec, Dir::BingxToDex, d("5"));
    p.bisect_steps = 2;
    let mut c = curve(&rec);
    match search(&mut c, &p).await {
        Err(Fail::TooSmall(t)) => {
            assert!(t.contains("ближайший посчитанный"), "{t}");
            assert!(!t.contains("следующий шаг"), "{t}");
            assert!(t.contains("больше бюджета 5"), "{t}");
        }
        other => panic!("ожидался TooSmall: {other:?}"),
    }
}
