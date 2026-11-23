//! Регрессии определения границы глубины и пересчёта в единицы покупки.
use super::*;

#[tokio::test]
async fn refit_shares_one_recovery_with_precheck() {
    let start = Instant::now();
    let (_, unused) = before_order(Some(start), async {
        assert_eq!(before_buy(), Some(start));
    })
    .await;
    let (_, spent) = before_order(unused, async {
        BEFORE_BUY.with(|slot| slot.set(None));
    })
    .await;
    before_order(spent, async {
        assert_eq!(before_buy(), None);
    })
    .await;
}

fn short(o: &mut Observation, leg: Leg, requested: Decimal, filled: Decimal) {
    assert!(matches!(
        check("B", leg, requested, filled, Some(o)),
        Err(Fail::TooLarge(_))
    ));
}
macro_rules! dec {
    ($n:literal) => {
        Decimal::from_str_exact(stringify!($n)).unwrap()
    };
}

#[test]
fn reserve_uses_original_requested_depth_and_applies_share_once() {
    let mut o = Observation {
        buy: dec!(100),
        gap: None,
    };
    for (filled, expected) in [(dec!(100), dec!(70)), (dec!(130), dec!(91))] {
        short(
            &mut o,
            Leg::Reserve { arrive: dec!(100) },
            dec!(100) / dec!(0.7),
            filled,
        );
        assert_eq!(
            o.candidate(dec!(0.7), dec!(0), dec!(1), dec!(1)),
            Some(expected)
        );
    }
    short(&mut o, Leg::Buy, dec!(100), dec!(75.9));
    assert_eq!(
        o.candidate(dec!(0.7), dec!(0), dec!(1), dec!(1)),
        Some(dec!(75))
    );
    short(
        &mut o,
        Leg::Sell { arrive: dec!(100) },
        dec!(100),
        dec!(75.9),
    );
    assert_eq!(
        o.candidate(dec!(0.7), dec!(0), dec!(1), dec!(1)),
        Some(dec!(75))
    );
    o.clear();
    assert_eq!(o.candidate(dec!(0.7), dec!(0), dec!(1), dec!(1)), None);
}

#[test]
fn invert_sale_quantity_with_fixed_fee_and_ten_percent_hold() {
    let mut o = Observation {
        buy: dec!(117),
        gap: None,
    };
    short(
        &mut o,
        Leg::Reserve {
            arrive: dec!(100.8),
        },
        dec!(100) / dec!(0.7),
        dec!(130),
    );
    assert_eq!(
        o.candidate(dec!(0.7), dec!(5), dec!(1), dec!(1)),
        Some(dec!(107))
    );
    assert_eq!(
        o.candidate(dec!(0.7), dec!(5), dec!(2), dec!(5)),
        Some(dec!(106))
    );
}

#[test]
fn recorded_cake_depth_loses_the_boundary_on_a_capped_requote() {
    use crate::gateway::frozen::walk;
    use crate::venues::bingx::depth::{Msg, parse};
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    let levels = raw
        .lines()
        .find_map(|line| {
            let row: serde_json::Value = serde_json::from_str(line).ok()?;
            match parse(row["m"].as_str()?) {
                Msg::Depth { symbol, bids, .. } if symbol == "CAKE-USDT" => Some(bids),
                _ => None,
            }
        })
        .unwrap();
    let depth: Decimal = levels.iter().map(|l| l.qty).sum();
    let h = dec!(0.7);
    let buy = (depth * dec!(0.9)).floor();
    let needed = buy / h;
    let (measured, _) = walk(&levels, needed);
    assert!(measured < needed);
    assert_eq!(walk(&levels, buy).0, buy);
    let mut o = Observation { buy, gap: None };
    short(&mut o, Leg::Reserve { arrive: buy }, needed, measured);
    let q = o
        .candidate(h, Decimal::ZERO, Decimal::ONE, Decimal::ONE)
        .unwrap();
    assert_eq!(walk(&levels, q / h).0, q / h);
    assert!(walk(&levels, (q + Decimal::ONE) / h).0 < (q + Decimal::ONE) / h);
}
