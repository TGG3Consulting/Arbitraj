//! Physical funding changes by a raw unit before or after a nominal fee boundary.
use super::*;

fn market(tokens: [Address; 2], reserves: [u64; 2], oracle: Option<(usize, U256)>) -> Market {
    let [a, b] = reserves.map(U256::from);
    Market {
        pool: Address::repeat_byte(7),
        tokens,
        tier: 10,
        reserves: [a, b],
        balances: [a, b, U256::ZERO],
        fee_reserve: U256::ZERO,
        supply: U256::from(1_000_000),
        oracle,
    }
}
fn book(embedded: bool) -> Book {
    let bun = Address::repeat_byte(1);
    let weth = Address::repeat_byte(2);
    let fee_pair = market([bun, weth], [1_000_000_000; 2], None);
    let main = if embedded {
        market([bun, weth], [1_000_000, 110_000], None)
    } else {
        market(
            [Address::repeat_byte(3), weth],
            [1_000_000_000; 2],
            Some((0, U256::ONE << 112)),
        )
    };
    Book {
        market: main,
        fee_pair,
        bun,
        weth,
        fee_rate_after: None,
        router_balance: U256::from(2_000_000),
        deposits: U256::from(2_000_000),
    }
}
#[test]
fn donation_requires_next_paid_fee_on_the_same_net_output() {
    let mut b = book(false);
    let input = b.market.tokens[0];
    let target = U256::from(9_800_076);
    let lower = U256::from(10_000_099);
    b.market.balances[0] += U256::ONE;
    assert!(b.exact_in(input, lower, false).is_none());
    assert_eq!(
        b.exact_in(input, lower + U256::ONE, false).unwrap().credit,
        target
    );
    let (s, _) = b.swap(input, false, target).unwrap();
    assert_eq!((s.amount_in, s.amount_out), (lower + U256::ONE, target));
    // One unit of actual external BUN already present covers the same donation.
    b.market.balances[2] = U256::ONE;
    assert_eq!(b.swap(input, false, target).unwrap().0.amount_in, lower);
    // A full extra fee unit is unavoidable at every input once the donation reaches 100.
    b.market.balances[0] = b.market.reserves[0] + U256::from(100);
    b.market.balances[2] = U256::ZERO;
    assert!(b.swap(input, false, target).is_none());
}
#[test]
fn embedded_balance_deficit_is_recovered_within_the_same_output_plateau() {
    let mut b = book(true);
    let input = b.bun;
    let n = U256::from(112_224);
    let target = U256::from(11_000);
    b.market.balances[0] -= U256::from(2);
    b.market.balances[2] = b.market.balances[0];
    assert!(b.exact_in(input, n, false).is_none());
    assert_eq!(
        b.exact_in(input, n + U256::ONE, false).unwrap().credit,
        target
    );
    assert_eq!(
        b.swap(input, false, target).unwrap().0.amount_in,
        n + U256::ONE
    );
    b.market.balances[1] -= U256::ONE;
    assert!(b.swap(input, false, target).is_none());
    assert!(b.swap(input, false, b.market.reserves[1]).is_none());
}

#[test]
fn fractional_oracle_can_cross_after_more_than_a_trillion_raw_units() {
    let q = U256::ONE << 112usize;
    let span = U256::ONE << 55usize;
    // rate = (span-1)/span; a donation of one basis unit is free only
    // when the nominal basis is a multiple of span.
    let rate = q - q / span;
    assert_eq!(
        first_covered(U256::ONE, U256::ONE, span, U256::ONE, rate, U256::ZERO),
        Some(span - U256::ONE)
    );
    assert_eq!(
        first_covered(
            U256::ONE,
            U256::ONE,
            span - U256::ONE,
            U256::ONE,
            rate,
            U256::ZERO
        ),
        None
    );
}
#[test]
fn fee_pair_donation_does_not_cancel_the_market_minimum() {
    let mut b = book(false);
    let token = b.market.tokens[0];
    b.fee_pair.balances[0] += U256::from(200_000);
    let target = U256::from(1);
    let result = b.swap(token, false, target).unwrap().0;
    assert_eq!(result.amount_in, U256::from(100_000));
    assert!(
        b.exact_in(token, result.amount_in - U256::ONE, false)
            .is_none()
    );
    assert!(result.amount_out >= target);
}

#[test]
fn nearly_parallel_reserve_curves_skip_a_trillion_unit_plateau() {
    let reserve = U256::from(10).pow(U256::from(24));
    let x = reserve + U256::from(100_000);
    let end = Market::gap_end(
        reserve * reserve,
        x,
        U256::ONE,
        reserve,
        reserve,
        U256::ZERO,
        reserve * U256::from(2),
    )
    .unwrap();
    assert!(end - x > U256::from(500_000_000_000u64));
    // Over this interval ceil(R²/x) = 2R+1−x and (x−1)(2R+1−x) < R².
    for t in [x, end] {
        let y = div_up(reserve * reserve, t).unwrap();
        assert_eq!(y, reserve * U256::from(2) + U256::ONE - t);
        assert!((t - U256::ONE) * y < reserve * reserve);
    }
}
