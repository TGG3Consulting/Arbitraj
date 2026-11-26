//! CandySwap удерживает часть выходного токена после проверки кривой V2.
//! Пара списывает весь gross, получатель получает gross − floor(gross × rate / 10_000).
use super::{swap_cl::Swapped, v2};
use alloy_primitives::U256;

const BASIS: u32 = 10_000;
pub(crate) const PAYOUT_RATE_SELECTOR: [u8; 4] = [0xc3, 0x59, 0x5a, 0x85];

pub(crate) fn net(gross: U256, rate: u16) -> Option<U256> {
    if u32::from(rate) > BASIS {
        return None;
    }
    gross.checked_sub(gross.checked_mul(U256::from(rate))? / U256::from(BASIS))
}

/// Наименьший gross, при котором ceil(gross * (BASIS-rate) / BASIS) достигнет net.
fn gross_for(net: U256, rate: u16) -> Option<U256> {
    let kept = BASIS.checked_sub(u32::from(rate))?;
    if kept == 0 || net.is_zero() {
        return None;
    }
    ((net - U256::ONE).checked_mul(U256::from(BASIS))? / U256::from(kept)).checked_add(U256::ONE)
}

pub(crate) fn quote(
    rin: U256,
    rout: U256,
    fee: (U256, U256),
    rate: u16,
    exact_in: bool,
    amount: U256,
) -> Option<(Swapped, U256)> {
    let input = if exact_in {
        amount
    } else {
        let gross = gross_for(amount, rate)?;
        if gross >= rout || rin.is_zero() {
            return None;
        }
        let denominator = (rout - gross).checked_mul(fee.0)?;
        if denominator.is_zero() {
            return None;
        }
        rin.checked_mul(gross)?
            .checked_mul(fee.1)?
            .div_ceil(denominator)
    };
    let gross = v2::amount_out(input, rin, rout, fee)?;
    let paid = net(gross, rate)?;
    if paid.is_zero() || (!exact_in && paid < amount) {
        return None;
    }
    Some((
        Swapped {
            amount_in: input,
            amount_out: if exact_in { paid } else { amount },
            complete: true,
        },
        gross,
    ))
}

/// Расход плательщика покрывает дефицит резерва. Чужой прежний остаток не оплачивает его обмен.
pub(crate) fn funded(
    market: &crate::core::types::CandySwapState,
    zero_in: bool,
    fee: (U256, U256),
    exact_in: bool,
    amount: U256,
) -> Option<(Swapped, crate::core::types::CandySwapState)> {
    let i = usize::from(!zero_in);
    let o = 1 - i;
    let (rin, rout) = (market.reserves[i], market.reserves[o]);
    if amount.is_zero() || market.balances[o] < rout {
        return None;
    }
    let wallet_in = if exact_in {
        amount
    } else {
        let needed = quote(rin, rout, fee, market.payout_bps, false, amount)?
            .0
            .amount_in;
        needed.checked_add(rin.saturating_sub(market.balances[i]))?
    };
    let new_in = market.balances[i].checked_add(wallet_in)?;
    let credited = wallet_in.min(new_in.checked_sub(rin)?);
    let (mut fill, gross) = quote(rin, rout, fee, market.payout_bps, true, credited)?;
    let new_out = market.balances[o].checked_sub(gross)?;
    let max = (U256::ONE << 112) - U256::ONE;
    if new_in > max || new_out > max || (!exact_in && fill.amount_out < amount) {
        return None;
    }
    fill.amount_in = wallet_in;
    if !exact_in {
        fill.amount_out = amount;
    }
    let mut next = market.clone();
    next.balances[i] = new_in;
    next.balances[o] = new_out;
    next.reserves = next.balances;
    Some((fill, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdrawal_rounding_and_inverse_use_the_first_reachable_size() {
        for rate in [0, 15, 3333, 9999, 10_000] {
            let fee = (U256::from(9985), U256::from(10_000));
            let (rin, rout) = (U256::from(1003), U256::from(2_000_003));
            for amount in [1, 2, 599, 666, 667, 1999, 10_000] {
                let n = U256::from(amount);
                if rate == 10_000 {
                    assert_eq!(net(n, rate), Some(U256::ZERO));
                    assert!(gross_for(n, rate).is_none());
                    continue;
                }
                let gross = gross_for(n, rate).unwrap();
                assert_eq!(net(gross, rate), Some(n));
                assert!(net(gross - U256::ONE, rate).unwrap() < n);
                if gross >= rout {
                    continue;
                }
                let (buy, _) = quote(rin, rout, fee, rate, false, n).unwrap();
                let (paid, _) = quote(rin, rout, fee, rate, true, buy.amount_in).unwrap();
                assert!(paid.amount_out >= n);
                assert!(
                    quote(rin, rout, fee, rate, true, buy.amount_in - U256::ONE)
                        .is_none_or(|(v, _)| v.amount_out < n)
                );
            }
        }
    }
    #[test]
    fn net_payments_and_protocol_deltas_match_each_base_wallet_receipt() {
        use crate::core::types::CandySwapState;
        use alloy_primitives::Address;
        use serde_json::Value;
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/candyswap_base.json"
        ))
        .unwrap();
        let n = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let mut legs = 0;
        for sample in fixture["net_payouts"].as_array().unwrap() {
            let pool = fixture["pools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["pool"] == sample["pool"])
                .unwrap();
            let reserves = [n(&pool["reserves"][0]), n(&pool["reserves"][1])];
            let zero = pool["tokens"][0] == "0x4200000000000000000000000000000000000006";
            let state = CandySwapState {
                reserves,
                balances: reserves,
                payout_bps: sample["transfer_fee"].as_u64().unwrap().try_into().unwrap(),
                receiver: sample["fee_receiver"]
                    .as_str()
                    .unwrap()
                    .parse::<Address>()
                    .unwrap(),
            };
            let fee = (
                U256::from(10000 - pool["commission"]["value"].as_u64().unwrap()),
                U256::from(10000),
            );
            for trial in sample["cases"].as_array().unwrap() {
                let mut market = state.clone();
                for (side, input0) in [("buy", zero), ("sell", !zero)] {
                    let observed = &trial[side];
                    let input = n(&observed["spent"]);
                    let output = usize::from(input0);
                    let (fill, next) = funded(&market, input0, fee, true, input).unwrap();
                    assert_eq!(fill.amount_out, n(&observed["paid"]));
                    assert_eq!(
                        market.balances[output] - next.balances[output],
                        n(&observed["gross"])
                    );
                    assert_eq!(n(&observed["gross"]) - fill.amount_out, n(&observed["cut"]));
                    let needed = funded(&market, input0, fee, false, fill.amount_out)
                        .unwrap()
                        .0
                        .amount_in;
                    assert!(needed <= input);
                    assert!(
                        funded(&market, input0, fee, true, needed - U256::ONE)
                            .is_none_or(|(f, _)| f.amount_out < fill.amount_out)
                    );
                    assert!(observed["strict"]["code"].is_number());
                    market = next;
                    legs += 1;
                }
            }
        }
        assert_eq!(legs, 12);
    }
}
