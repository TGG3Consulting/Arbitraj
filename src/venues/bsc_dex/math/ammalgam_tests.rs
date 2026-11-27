//! Границы глубины, округление комиссии и котировки из архива десяти рынков Ethereum.
use super::*;
use std::io::Read;
#[test]
fn depth_matches_an_atom_sweep_across_both_piecewise_reserve_boundaries() {
    let w = U256::from;
    let buffer = 20_u128;
    let adjusted = |r: u128, m: u128| r.min(buffer * (r - m));
    for (rin, rout) in [(20_u128, 40_u128), (40, 20), (55, 76)] {
        for mi in [0, rin * 19 / 20, rin - 1] {
            for mo in [0, rout * 19 / 20, rout - 1] {
                let x = adjusted(rin, mi);
                let y = adjusted(rout, mo);
                let k = x * y;
                let si = if x < rin { buffer } else { 1 };
                let so = if (rout - mo) * buffer <= rout {
                    buffer
                } else {
                    1
                };
                for cash in [1, rout / 2, rout] {
                    let cap = cash.min(rout - mo - 1);
                    let state = AmmalgamState {
                        reserves: [w(rin), w(rout)],
                        reference: [w(rin), w(rout)],
                        missing: [w(mi), w(mo)],
                        cash: [w(rin), w(cash)],
                    };
                    for pct in [1, 20, 75, 90] {
                        for buy in [true, false] {
                            let (top, bottom) = if buy {
                                (100 + pct, 100)
                            } else {
                                (100, 100 - pct)
                            };
                            let mut depth = 0;
                            for dx in 1..=800 {
                                let rx = rin + dx;
                                let ax = adjusted(rx, mi);
                                // Left-hand derivative at the terminal point: the whole
                                // continuous interval up to it must stay within the band.
                                let sx = if buffer * (rx - mi) <= rx { buffer } else { 1 };
                                let sy = if k * (buffer - 1) < ax * buffer * mo {
                                    buffer
                                } else {
                                    1
                                };
                                if x * x * sx * so * top < ax * ax * sy * si * bottom {
                                    break;
                                }
                                let ay = k.div_ceil(ax);
                                let output = rout - ay.max(mo + ay.div_ceil(buffer));
                                if output > 0 && cap > 0 {
                                    depth = if buy { dx } else { output.min(cap) };
                                }
                                if output >= cap {
                                    break;
                                }
                            }
                            let actual = state.band(0, buy, w(pct), w(100)).unwrap().0;
                            assert_eq!(
                                actual,
                                w(depth),
                                "{rin}/{rout}, debt {mi}/{mo}, cash {cash}, band {pct}, buy {buy}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn physical_output_caps_the_protected_payment_after_rounding() {
    let state = AmmalgamState {
        reserves: [U256::from(100), U256::from(1_000_000)],
        reference: [U256::from(100), U256::from(1_000_000)],
        cash: [U256::from(100), U256::from(1000)],
        missing: [U256::ZERO; 2],
    };
    assert_eq!(
        state.band(0, false, U256::from(3), U256::from(100)),
        Some((U256::from(1000), false))
    );
    assert!(state.swap(true, true, U256::ONE).is_none());
    assert_eq!(
        state.swap(true, true, U256::from(2)).unwrap().amount_out,
        U256::from(1000)
    );
    assert_eq!(
        state.swap(true, false, U256::from(1000)).unwrap().amount_in,
        U256::from(2)
    );
}

#[test]
fn rounded_fee_can_lower_the_payout_when_input_increases_by_one() {
    // Both calls recorded from computeExpectedSwapAmountOut on Ethereum. The reserve
    // arrangement fits the deployed ABI and deliberately lands on a Q64 fee step.
    let reserve = U256::ONE << 110;
    let state = AmmalgamState {
        reserves: [reserve; 2],
        reference: [reserve; 2],
        missing: [U256::ZERO; 2],
        cash: [reserve; 2],
    };
    let breakpoint = U256::ONE << 109;
    let target = U256::from(370878347038201973466465818636745u128);
    assert_eq!(state.payout(0, breakpoint - U256::ONE), Some(target));
    assert_eq!(
        state.payout(0, breakpoint),
        Some(U256::from(370878347038201973466464023515720u128))
    );
    let minimum = state.swap(true, false, target).unwrap().amount_in;
    assert!(minimum < breakpoint && state.payout(0, minimum).unwrap() >= target);
    assert!(state.payout(0, minimum - U256::ONE).unwrap() < target);
}

#[test]
fn integer_cash_curve_matches_the_verified_helper_on_all_ten_mainnet_markets() {
    let mut tape = Vec::new();
    flate2::read::GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ammalgam_ethereum_quotes.json.gz"
        )
        .as_slice(),
    )
    .read_to_end(&mut tape)
    .unwrap();
    let data: serde_json::Value = serde_json::from_slice(&tape).unwrap();
    let number = |v: &serde_json::Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let mut mismatches = Vec::new();
    let (mut equal, mut over, mut short) = (0, 0, 0);
    for row in data["rows"].as_array().unwrap() {
        let s = &row["state"];
        let market = AmmalgamState {
            reserves: [number(&s[0]), number(&s[2])],
            reference: [number(&s[1]), U256::ZERO],
            missing: [number(&s[3]), number(&s[4])],
            cash: [U256::ZERO, number(&row["output_cash"])],
        };
        for saved in row["inverse"].as_array().unwrap() {
            let target = number(&saved["target"]);
            let suggestion = number(&saved["suggested_input"]);
            let fill = market
                .swap(true, false, target)
                .expect("funded reverse quote");
            let paid = fill.amount_in;
            assert!(
                market.payout(0, paid).unwrap() >= target
                    && market.payout(0, paid - U256::ONE).unwrap() < target
            );
            match suggestion.cmp(&paid) {
                std::cmp::Ordering::Equal => equal += 1,
                std::cmp::Ordering::Greater => {
                    over += 1;
                }
                std::cmp::Ordering::Less => {
                    assert!(market.payout(0, suggestion).unwrap() < target);
                    short += 1;
                }
            }
        }
        for quote in row["quotes"].as_array().unwrap() {
            let amount = number(&quote["amount"]);
            let actual = market.payout(0, amount);
            let expected = number(&quote["quote"]);
            if actual != Some(expected) {
                mismatches.push((row["pool"].as_str().unwrap(), amount, actual, expected));
            }
        }
    }
    assert_eq!(equal + over + short, 135);
    if let Some(path) = std::env::var_os("ARB_AMMALGAM_INVERSE_EXPORT") {
        let result = serde_json::json!({"block":26494988,"forward":185,"inverse":equal+over+short,
            "minimum_matches_helper":equal,"helper_input_above_minimum":over,"helper_input_below_minimum":short});
        std::fs::write(path, serde_json::to_vec(&result).unwrap()).unwrap();
    }
    assert!(
        mismatches.is_empty(),
        "{} quotes differ; first three: {:?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(3)]
    );
}
