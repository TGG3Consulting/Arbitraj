//! Mirrors AntfarmAtfPair::_update followed by AntfarmOracle::update on a 1% fee market.
use alloy_primitives::U256;

pub(super) struct Observation {
    pub pair_cumulative: U256,
    pub oracle_cumulative: U256,
    pub pair_time: u32,
    pub oracle_time: u32,
    pub average: U256,
    pub first: bool,
}
impl Observation {
    pub fn rate(&self, reserves: [U256; 2], now: u32) -> Option<U256> {
        // The pair uses checked addition in Solidity 0.8.10. The separate oracle wraps subtraction.
        let mut cumulative = self.pair_cumulative;
        let pair_elapsed = now.wrapping_sub(self.pair_time);
        if pair_elapsed != 0 && !reserves.contains(&U256::ZERO) {
            let unit: U256 = (reserves[0] << 112) / reserves[1];
            cumulative = cumulative.checked_add(unit.checked_mul(U256::from(pair_elapsed))?)?;
        }
        let elapsed = now.wrapping_sub(self.oracle_time);
        if elapsed < 3600 && !self.first {
            return Some(self.average);
        }
        let avg = cumulative
            .wrapping_sub(self.oracle_cumulative)
            .checked_div(U256::from(elapsed))?;
        Some(avg & ((U256::ONE << 224) - U256::ONE))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::check::sim_base::Base;
    #[test]
    fn pricing_and_reverse_probe_agree_at_first_eligible_second() {
        let observed = Observation {
            pair_cumulative: U256::ZERO,
            oracle_cumulative: U256::ZERO,
            pair_time: 200,
            oracle_time: 200,
            average: U256::from(7),
            first: false,
        };
        let reserves = [U256::from(4), U256::from(2)];
        assert_eq!(observed.rate(reserves, 3799), Some(U256::from(7)));
        let at = Base::timed(12, 3799).time.unwrap() as u32;
        assert_eq!(observed.rate(reserves, at), Some(U256::from(2) << 112));
        let initializing = Observation {
            first: true,
            ..observed
        };
        assert_eq!(initializing.rate(reserves, 200), None);
        assert_eq!(initializing.rate(reserves, 201), Some(U256::from(2) << 112));
    }
    #[test]
    fn clock_wraps_but_pair_accumulator_is_checked_even_inside_the_window() {
        let mut o = Observation {
            pair_cumulative: U256::ZERO,
            oracle_cumulative: U256::ZERO,
            pair_time: u32::MAX,
            oracle_time: u32::MAX,
            average: U256::ZERO,
            first: true,
        };
        assert_eq!(o.rate([U256::ONE; 2], 0), Some(U256::ONE << 112));
        o.pair_cumulative = U256::MAX;
        o.first = false;
        assert_eq!(o.rate([U256::ONE; 2], 0), None);
    }
}
