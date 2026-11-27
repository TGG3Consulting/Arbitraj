//! Measured swap levels of a Curve lending AMM. Each point records money actually exchanged.
//! LLAMMA.get_dxdy can consume less than the offered input at its last reachable band.
//! https://docs.curve.finance/developer/integration/llamma/
use super::LadderPoint;
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub factory: Address,
    pub index: u64,
    pub amm: Address,
    pub runtime: B256,
    pub controller: Address,
    pub coins: [Address; 2],
    pub levels: [Vec<LadderPoint>; 2],
    /// Largest offered amount for which a partial debit was physically quoted.
    pub partial_cap: [Option<U256>; 2],
}
impl State {
    pub fn side(&self, input: Address) -> Option<usize> {
        self.coins.iter().position(|&a| a == input)
    }
    pub fn points(&self, input: Address) -> Option<&[LadderPoint]> {
        Some(&self.levels[self.side(input)?])
    }
}
