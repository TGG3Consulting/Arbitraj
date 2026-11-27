//! The canonical FeeRouter charges the two terminal creators and protocol fee once.
//! Put that fee at the input, output or first interior quote leg before solving the route.
use super::route_calc::Leg;
use crate::core::types::PoolState;
use alloy_primitives::Address;

pub(super) fn fees(legs: &mut [Leg]) -> Option<()> {
    let side = |l: &Leg, token: Address| {
        let PoolState::Motoswap(s) = &l.entry.pool.state else {
            return None;
        };
        let i = if token == l.entry.pool.token0 {
            0
        } else if token == l.entry.pool.token1 {
            1
        } else {
            return None;
        };
        Some(s.sides?[i])
    };
    let first = legs.first()?;
    let last = legs.last()?;
    let (creator_in, rank_in) = side(first, first.token_in)?;
    let (creator_out, rank_out) = side(last, last.token_out)?;
    let (charged_leg, at_output) = match (rank_in, rank_out) {
        (None, None) => {
            let j = legs
                .iter()
                .position(|l| side(l, l.token_out).is_some_and(|(_, r)| r.is_some()))?;
            if j + 1 == legs.len() {
                return None;
            }
            (j, true)
        }
        (_, Some(_)) if rank_out >= rank_in => (legs.len() - 1, true),
        _ => (0, false),
    };
    let PoolState::Motoswap(canonical) = &first.entry.pool.state else {
        return None;
    };
    let (fee, creator) = (canonical.protocol_bps, canonical.creator_bps);
    for (index, leg) in legs.iter_mut().enumerate() {
        let PoolState::Motoswap(s) = &mut leg.entry.pool.state else {
            return None;
        };
        if (s.protocol_bps, s.creator_bps) != (fee, creator) {
            return None;
        }
        if index == charged_leg {
            s.creators = u8::from(creator_in) + u8::from(creator_out);
            s.fee_on_output[usize::from(leg.token_in != leg.entry.pool.token0)] = at_output;
        } else {
            s.protocol_bps = 0;
            s.creator_bps = 0;
            s.creators = 0;
        }
    }
    Some(())
}
