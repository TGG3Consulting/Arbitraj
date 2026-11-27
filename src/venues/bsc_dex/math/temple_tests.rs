use super::*;

fn market(r0: u128, r1: u128, n: u128, d: u128) -> TempleState {
    TempleState {
        pair: Address::repeat_byte(10),
        router: Address::repeat_byte(11),
        temple: Address::repeat_byte(1),
        stable: Address::repeat_byte(2),
        defend: Address::repeat_byte(3),
        reserves: [r0.into(), r1.into()],
        balances: [r0.into(), r1.into()],
        ratio: Some([n.into(), d.into()]),
        defense_balance: U256::from(1_000_000),
        amm_authorized: true,
    }
}

#[test]
fn historical_switch_preserves_both_assets_and_the_last_frax_fill() {
    let mut s = market(1162525590459825404423, 2915003405983751896683, 8000, 10000);
    s.defense_balance = U256::ZERO;
    for dx in [
        2475386829881875394045u128,
        2475386829881875394046,
        2475386829881875394047,
    ] {
        let x = U256::from(dx);
        assert!(paid(&s, s.temple, s.stable, x).is_some());
        assert!(paid(&s, s.temple, s.defend, x).is_none());
    }
    let next = U256::from(2475386829881875394048u128);
    assert!(paid(&s, s.temple, s.stable, next).is_none());
    assert!(paid(&s, s.temple, s.defend, next).is_none());
    let expected = U256::from(1980309463905500315238u128);
    s.defense_balance = expected;
    s.amm_authorized = false;
    let (f, post) = paid(&s, s.temple, s.defend, next).unwrap();
    assert_eq!(
        (f.amount_out, post.defense_balance, post.reserves),
        (expected, U256::ZERO, s.reserves)
    );
    assert!(paid(&s, s.defend, s.temple, expected).is_none());
    assert_eq!(
        quote(&s, s.temple, s.defend, false, expected)
            .unwrap()
            .amount_in,
        next
    );
}

#[test]
fn exact_output_uses_least_executable_input_across_rounding_and_same_asset_settlement() {
    for (r0, r1, n, d) in [
        (20, 50, 4, 5),
        (32, 32, 1, 2),
        (7, 17, 5, 3),
        (30, 40, 0, 7),
        (61, 67, 13, 28),
    ] {
        let mut s = market(r0, r1, n, d);
        for same in [false, true] {
            if same {
                s.defend = s.stable;
            }
            for (a, b) in [
                (s.temple, s.stable),
                (s.temple, s.defend),
                (s.stable, s.temple),
            ] {
                for target in 1..=45u64 {
                    let want = U256::from(target);
                    let expected = (1..=150u64).find_map(|i| {
                        let (f, _) = paid(&s, a, b, U256::from(i))?;
                        (f.amount_out >= want).then_some(f)
                    });
                    let actual = quote(&s, a, b, false, want);
                    assert_eq!(
                        expected,
                        actual.filter(|f| f.amount_in <= U256::from(150)),
                        "{r0}/{r1} {n}/{d} {same} {target}"
                    );
                }
            }
        }
    }
}

#[test]
fn buy_does_not_require_iv_and_pool_gifts_do_not_inflate_its_quote() {
    let mut s = market(10000, 10000, 1, 1);
    s.ratio = None;
    let (before, _) = paid(&s, s.stable, s.temple, U256::from(100)).unwrap();
    s.balances[0] += U256::from(50);
    let (after, new) = paid(&s, s.stable, s.temple, U256::from(100)).unwrap();
    assert_eq!(before, after);
    assert_eq!(new.reserves[0], s.balances[0] - after.amount_out);
    assert!(paid(&s, s.temple, s.stable, U256::from(100)).is_none());
}

#[test]
fn near_parallel_prices_skip_hundreds_of_billions_of_tied_raw_inputs() {
    let scale = 10u128.pow(20);
    let denominator = 10u128.pow(30);
    let mut s = market(995 * scale, 1000 * scale, denominator - 1, denominator);
    s.defense_balance = U256::from(denominator - 1);
    let x = quote(&s, s.temple, s.defend, false, U256::ONE)
        .unwrap()
        .amount_in;
    // On this interval IV=floor(x*(d-1)/d)=x-1 and AMM=floor(R*x/(R+x)).
    // Hence treasury wins for the first time exactly when x*(x-1)>R.
    let r = U256::from(1000 * scale);
    assert!(x * (x - U256::ONE) > r && (x - U256::ONE) * (x - U256::from(2)) <= r);
    assert!(x > U256::from(100_000_000_000u64));
}
