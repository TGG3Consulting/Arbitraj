//! Physical Ambient pairs live in the CrocSwap singleton. Native ETH has address zero in the pool key.
use super::LadderPoint;
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Market {
    pub base: Address,
    pub quote: Address,
    pub index: U256,
}
impl Market {
    pub fn id(self) -> B256 {
        keccak256((self.base, self.quote, self.index).abi_encode())
    }
    pub fn tokens(self, wrapped: Address) -> Option<[Address; 2]> {
        if self.base >= self.quote || wrapped.is_zero() {
            return None;
        }
        let base = if self.base.is_zero() {
            wrapped
        } else {
            self.base
        };
        (base != self.quote).then_some([base, self.quote])
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub module: Option<Address>,
    pub dex: Address,
    pub market: Market,
    pub wrapped: Address,
    pub impact: Address,
    pub levels: [Vec<LadderPoint>; 2],
    pub partial_cap: [Option<U256>; 2],
}
impl State {
    pub fn side(&self, input: Address) -> Option<usize> {
        self.market
            .tokens(self.wrapped)?
            .iter()
            .position(|a| *a == input)
    }
    pub fn points(&self, input: Address) -> Option<&[LadderPoint]> {
        Some(&self.levels[self.side(input)?])
    }
}
