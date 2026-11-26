//! Мемобокс применяет floor к комиссии прихода до проверки произведения доступных резервов.
use super::swap_cl::Swapped;
use crate::core::types::MemeboxState;
use alloy_primitives::U256;

const BPS: u64 = 10_000;

fn forward(s: &MemeboxState, i: usize, amount: U256) -> Option<(Swapped, MemeboxState)> {
    let o = 1 - i;
    let (ri, ro) = (s.reserves[i], s.reserves[o]);
    let (bi, bo) = (s.available[i], s.available[o]);
    let max = U256::from(u128::MAX >> 16);
    if amount.is_zero()
        || ri.is_zero()
        || ro <= U256::ONE
        || ri > max
        || ro > max
        || u64::from(s.fee_bps) > BPS
        || u64::from(s.protocol_bps) > BPS
        || s.physical[i] < bi
        || s.physical[o] < bo
    {
        return None;
    }
    let paid = bi.checked_add(amount)?;
    let credited = paid.checked_sub(ri)?;
    let divider = U256::from(BPS);
    let rate = U256::from(s.fee_bps);
    let fee = (credited / divider) * rate + ((credited % divider) * rate) / divider;
    let adjusted = paid.checked_sub(fee)?;
    if adjusted > max {
        return None;
    }
    let keep = (ri * ro).div_ceil(adjusted);
    let own_fee = (amount / divider) * rate + ((amount % divider) * rate) / divider;
    let own_adjusted = ri.checked_add(amount - own_fee)?;
    let own = ro - (ri * ro).div_ceil(own_adjusted);
    let gross = bo.checked_sub(keep)?.min(own);
    if gross.is_zero() || bo - gross > max {
        return None;
    }
    let mut next = s.clone();
    next.reserves[i] = adjusted;
    next.reserves[o] = bo - gross;
    next.available = next.reserves;
    next.physical[i] = s.physical[i].checked_add(amount)?;
    next.physical[o] = s.physical[o].checked_sub(gross)?;
    Some((
        Swapped {
            amount_in: amount,
            amount_out: gross,
            complete: true,
        },
        next,
    ))
}

pub(crate) fn quote(
    s: &MemeboxState,
    zero_in: bool,
    exact_in: bool,
    amount: U256,
) -> Option<(Swapped, MemeboxState)> {
    let i = usize::from(!zero_in);
    if exact_in {
        return forward(s, i, amount);
    }
    let o = 1 - i;
    let (ri, ro) = (s.reserves[i], s.reserves[o]);
    let bo = s.available[o];
    let max = U256::from(u128::MAX >> 16);
    let want = amount.max(bo.saturating_sub(max));
    if amount.is_zero() || want >= ro || want >= bo || ri > max || ro > max {
        return None;
    }
    let own_net = (ri * ro).div_ceil(ro - want).saturating_sub(ri);
    let actual_net = (ri * ro).div_ceil(bo - want).saturating_sub(ri);
    let funded = |net: U256| -> Option<U256> {
        if net.is_zero() {
            return Some(U256::ZERO);
        }
        let kept = BPS.checked_sub(u64::from(s.fee_bps))?;
        if kept == 0 {
            return None;
        }
        Some((net - U256::ONE).checked_mul(U256::from(BPS))? / U256::from(kept) + U256::ONE)
    };
    let input = ri
        .checked_add(funded(actual_net)?)?
        .saturating_sub(s.available[i])
        .max(funded(own_net)?);
    let (mut result, next) = forward(s, i, input)?;
    if result.amount_out < amount {
        return None;
    }
    result.amount_out = amount;
    Some((result, next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn n(v: &Value) -> U256 {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
            .parse()
            .unwrap()
    }

    #[test]
    fn archive_wallet_limits_and_smallest_exact_output_input() {
        let j: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/memebox_base.json"
        ))
        .unwrap();
        for sample in j["cases"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| [(&p["pool"], &p["buy"]), (&p["pool"], &p["sell"])])
            .chain(
                j["imbalanced"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (&p["pool"], &p["leg"])),
            )
        {
            let (pool, leg) = sample;
            let m = j["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| &x["pool"] == pool)
                .unwrap();
            let z = m["tokens"][0] == leg["input_token"];
            let (i, o) = (usize::from(!z), usize::from(z));
            let b = &leg["before"];
            let mut physical = [U256::ZERO; 2];
            physical[i] = n(&b["token_in"]);
            physical[o] = n(&b["token_out"]);
            let s = MemeboxState {
                reserves: [n(&b["reserves"][0]), n(&b["reserves"][1])],
                available: [n(&b["balances"][0]), n(&b["balances"][1])],
                physical,
                fee_bps: m["fee"].as_u64().unwrap() as u16,
                protocol_bps: m["protocol_ratio"].as_u64().unwrap() as u16,
            };
            let input = n(&leg["pair_input"]);
            let (fill, after) = quote(&s, z, true, input).unwrap();
            let credit = input - input * U256::from(s.fee_bps) / U256::from(BPS);
            let own = s.reserves[o] * credit / (s.reserves[i] + credit);
            assert_eq!(fill.amount_out, n(&leg["gross"]).min(own));
            assert_eq!(after.physical[i] - s.physical[i], input);
            assert_eq!(s.physical[o] - after.physical[o], fill.amount_out);
            let minimum = quote(&s, z, false, fill.amount_out).unwrap().0.amount_in;
            assert!(minimum <= input);
            if minimum > U256::ONE {
                assert!(
                    quote(&s, z, true, minimum - U256::ONE)
                        .is_none_or(|(x, _)| x.amount_out < fill.amount_out)
                );
            }
            if own >= n(&leg["gross"]) {
                assert_eq!(
                    after.reserves,
                    [
                        n(&leg["after"]["reserves"][0]),
                        n(&leg["after"]["reserves"][1])
                    ]
                );
            }
        }
    }

    #[test]
    fn protected_wallet_results_match_the_first_run_module() {
        let wallet: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/memebox_base_module.json"
        ))
        .unwrap();
        let first: Value =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        let profile = &first
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == "memebox-base")
            .unwrap()["settings"];
        assert_eq!(
            profile["dex.module_runtime_hashes"][0],
            wallet["module_hash"]
        );
        assert!(profile["dex.router"].is_null());
        let pairs = wallet["cases"].as_array().unwrap();
        assert_eq!(pairs.len(), 8);
        for leg in pairs.iter().flat_map(|x| [&x["buy"], &x["sell"]]) {
            assert_eq!(n(&leg["paid"]), n(&leg["gross"]));
            assert_eq!(n(&leg["pair_limit"]), n(&leg["gross"]));
            assert!(leg["above"].is_object());
        }
        let donated = wallet["imbalanced"].as_array().unwrap();
        assert_eq!(donated.len(), 8);
        for row in donated {
            let leg = &row["leg"];
            assert!(n(&leg["gross"]) < n(&leg["pair_limit"]));
            assert_eq!(n(&leg["paid"]), n(&leg["gross"]));
            assert!(leg["above"].is_object());
        }
    }

    #[test]
    fn available_deficit_reduces_payout_in_both_directions() {
        let mut s = MemeboxState {
            reserves: [U256::from(1000); 2],
            available: [U256::from(990), U256::from(980)],
            physical: [U256::from(1010); 2],
            fee_bps: 100,
            protocol_bps: 3000,
        };
        for z in [true, false] {
            let (fill, next) = quote(&s, z, true, U256::from(100)).unwrap();
            assert!(fill.amount_out > U256::ZERO && fill.amount_out < U256::from(90));
            assert!(quote(&s, z, true, U256::from(5)).is_none());
            assert!(
                next.available
                    .iter()
                    .zip(next.physical)
                    .all(|(a, b)| *a <= b)
            );
        }
        s.available = s.reserves;
        assert_eq!(
            quote(&s, true, true, U256::from(99)).unwrap().0.amount_out,
            U256::from(90)
        );
        assert_eq!(
            quote(&s, true, true, U256::from(100)).unwrap().0.amount_out,
            U256::from(90)
        );
    }
}
