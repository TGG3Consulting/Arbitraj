use super::*;
use crate::core::types::CarbonStrategy;
fn pair(orders: &[(u128, u64, u64)], fee: u32) -> CarbonState {
    let p = CarbonOrder {
        y: 0,
        z: 0,
        a: 0,
        b: 0,
    };
    CarbonState {
        controller: Address::repeat_byte(7),
        tokens: [Address::repeat_byte(1), Address::repeat_byte(2)],
        native_side: None,
        fee_ppm: fee,
        strategies: orders
            .iter()
            .enumerate()
            .map(|(i, &(y, a, b))| CarbonStrategy {
                id: U256::from(i + 1),
                orders: [p, CarbonOrder { y, z: y, a, b }],
            })
            .collect(),
    }
}

#[test]
fn split_two_curves_beats_either_full_size_and_respects_each_strategy_balance() {
    let q = 1u64 << 48;
    // A=1 and B=1 are encoded with exponent=1, mantissa=ONE/2.
    let one = q + q / 2;
    let s = pair(&[(120, one, one), (120, one, one)], 2000);
    for by_target in [false, true] {
        let n = 80;
        let plans = alternatives(&s, s.tokens[0], by_target, n);
        let p = &plans[0];
        let brute = (0..=n).filter_map(|i| {
            let f = |x| {
                if x == 0 {
                    Some(0)
                } else {
                    amount(&s.strategies[0].orders[1], by_target, x)
                }
            };
            let total = with_fee(f(i)? + f(n - i)?, by_target, 2000)?;
            Some(total)
        });
        let best = if by_target { brute.min() } else { brute.max() }.unwrap();
        assert_eq!(if by_target { p.spent } else { p.received }, best);
        let post = after(&s, s.tokens[0], p).unwrap();
        let left = post.strategies.iter().map(|r| r.orders[1].y).sum::<u128>();
        assert_eq!(
            240 - left,
            if by_target {
                p.received
            } else {
                p.received + p.fee
            }
        );
        assert_eq!(p.actions.len(), 2);
    }
}

#[test]
fn fee_rounds_once_and_unavailable_inventory_is_rejected() {
    let q = 1u64 << 48;
    let one = q + q / 2;
    let s = pair(&[(5, 0, one), (5, 0, one)], 100000);
    let p = plan(&s, s.tokens[0], false, 10).unwrap();
    assert_eq!((p.spent, p.received, p.fee), (10, 9, 1));
    assert!(plan(&s, s.tokens[0], true, 11).is_none());
    assert!(plan(&s, s.tokens[1], false, 1).is_none());
    assert!(plan(&s, Address::ZERO, false, 1).is_none());
}
