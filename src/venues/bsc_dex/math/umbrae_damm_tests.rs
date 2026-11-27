//! Регрессии округления и физического обеспечения Umbrae DAMM.
use super::*;

fn n(x: u64) -> U256 {
    U256::from(x)
}

fn market() -> UmbraeDammState {
    UmbraeDammState {
        tokens: [Address::repeat_byte(2), Address::repeat_byte(1)],
        reserves: [n(10_000), n(10_000)],
        balances: [n(10_017), n(10_000)],
        fee_side: 0,
        base_fee: 100,
        max_fee: 150,
        protocol_share: 2500,
        volatility: 0,
        last_price: n(PRICE_SCALE),
        protocol_fees: n(7),
        lp_fees: n(10),
        lp_supply: n(100),
        lp_index: n(3),
    }
}

#[test]
fn paid_input_output_and_next_fee_follow_pair_balances() {
    let s = market();
    let f = fill(&s, s.tokens[0], true, n(1000)).unwrap();
    assert_eq!((f.fee, f.swapped.amount_out), (n(10), n(900)));
    assert_eq!(f.after.balances, [n(11017), n(9100)]);
    assert_eq!(f.after.reserves, [n(10990), n(9100)]);
    assert_eq!(
        (f.after.protocol_fees, f.after.lp_fees, f.after.lp_index),
        (n(9), n(18), n(3) + n(8) * n(PRICE_SCALE) / n(100))
    );
    assert_eq!(f.after.last_price, n(9100) * n(PRICE_SCALE) / n(10990));
    assert_eq!(f.after.volatility, 1719);
    assert_eq!(rate(&f.after), Some(117));
    let reverse = fill(&f.after, s.tokens[1], true, n(1000)).unwrap();
    let gross = n(1000) * n(10990) / n(10100);
    assert_eq!(reverse.fee, gross * n(117) / n(10000));
    assert_eq!(reverse.swapped.amount_out, gross - reverse.fee);
    assert_eq!(reverse.after.reserves, [n(10990) - gross, n(10100)]);
}

#[test]
fn donations_sync_only_after_the_original_payout() {
    let s = market();
    let base = fill(&s, s.tokens[1], true, n(1000)).unwrap();
    let mut changed = s.clone();
    changed.balances[0] += n(50_000);
    let f = fill(&changed, s.tokens[1], true, n(1000)).unwrap();
    assert_eq!(base.swapped, f.swapped);
    assert_eq!(base.fee, f.fee);
    assert_eq!(base.after.reserves[0] + n(50_000), f.after.reserves[0]);
    assert_eq!(f.after.volatility, 10000);
    assert_eq!(rate(&f.after), Some(150));
    changed.balances[0] = n(100);
    assert!(fill(&changed, s.tokens[1], true, n(1000)).is_none());
    assert!(fill(&changed, s.tokens[1], true, n(60)).is_some());
    assert_eq!(band(&changed, 1, n(500), false), Some((n(83), false)));
}

#[test]
fn inverses_are_minimal_in_both_fee_denominations() {
    let mut s = market();
    for bps in [0, 25, 500, 9999, 10000] {
        s.base_fee = bps;
        s.max_fee = bps;
        for side in 0..2 {
            for target in [1, 2, 13, 399, 400, 9900] {
                let pay = fill(&s, s.tokens[side], false, n(target));
                if bps == 10000
                    || (side != s.fee_side && gross(n(target), bps).unwrap() >= s.reserves[0])
                {
                    assert!(pay.is_none());
                    continue;
                }
                let f = pay.unwrap();
                assert!(f.swapped.amount_out >= n(target));
                assert!(
                    fill(&s, s.tokens[side], true, f.swapped.amount_in - U256::ONE)
                        .is_none_or(|p| p.swapped.amount_out < n(target))
                );
            }
        }
    }
}

#[test]
fn zero_supply_accrues_to_protocol_and_math_rejects_evm_overflow() {
    let mut s = market();
    s.lp_supply = U256::ZERO;
    let f = fill(&s, s.tokens[0], true, n(1000)).unwrap();
    assert_eq!(
        (f.after.lp_index, f.after.lp_fees, f.after.protocol_fees),
        (s.lp_index, s.lp_fees, n(17))
    );
    assert!(fill(&s, s.tokens[0], true, U256::MAX).is_none());
    s.lp_supply = n(1);
    s.lp_index = U256::MAX;
    assert!(fill(&s, s.tokens[0], true, n(1000)).is_none());
    s.fee_side = 2;
    assert!(fill(&s, s.tokens[0], true, U256::ONE).is_none());
}
