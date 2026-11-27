//! Fluid DexLite directory slots store token0, token1 and salt. Its trading identity is bytes8(keccak256(abi.encode(key))).
use super::{LadderPoint, ambient::Stamp};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Market {
    pub token0: Address,
    pub token1: Address,
    pub salt: B256,
    pub index: U256,
}
impl Market {
    pub fn id(self) -> Option<B256> {
        if self.index.bit_len() > 192 {
            return None;
        }
        let hash = keccak256((self.token0, self.token1, self.salt).abi_encode());
        let mut id = self.index.to_be_bytes::<32>();
        id[..8].copy_from_slice(&hash[..8]);
        Some(id.into())
    }
    pub fn tokens(self, wrapped: Address) -> Option<[Address; 2]> {
        let normalize = |a| {
            if a == Address::repeat_byte(0xee) {
                wrapped
            } else {
                a
            }
        };
        let pair = [normalize(self.token0), normalize(self.token1)];
        (self.token0 != self.token1
            && !pair[0].is_zero()
            && !pair[1].is_zero()
            && pair[0] != pair[1])
            .then_some(pair)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub stamp: Stamp,
    pub dex: Address,
    pub resolver: Address,
    pub market: Market,
    pub wrapped: Address,
    pub module: Option<Address>,
    pub levels: [Vec<LadderPoint>; 2],
}
impl State {
    pub fn side(&self, input: Address) -> Option<usize> {
        self.market
            .tokens(self.wrapped)?
            .iter()
            .position(|v| *v == input)
    }
    pub fn points(&self, input: Address) -> Option<&[LadderPoint]> {
        self.stamp
            .current()
            .then_some(self.levels[self.side(input)?].as_slice())
    }
}
impl super::PoolState {
    pub fn singleton_stamp(&self) -> Option<&Stamp> {
        match self {
            Self::Ambient(s) => Some(&s.stamp),
            Self::FluidDexLite(s) => Some(&s.stamp),
            _ => None,
        }
    }
}
