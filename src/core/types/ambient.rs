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
/// Связь всех снимков площадки с последним наблюдавшимся каноническим заголовком.
#[derive(Clone, Debug, Default)]
pub struct Clock(std::sync::Arc<std::sync::RwLock<Option<(u64, B256)>>>);
impl Clock {
    pub fn observed(&self) -> Option<Stamp> {
        let (n, h) = (*self.0.read().unwrap_or_else(|e| e.into_inner()))?;
        Some(self.stamp(n, h))
    }
    pub fn advance(&self, tip: Option<(u64, B256)>) -> bool {
        let mut w = self.0.write().unwrap_or_else(|e| e.into_inner());
        if *w == tip {
            return false;
        }
        *w = tip;
        true
    }
    pub fn stamp(&self, number: u64, hash: B256) -> Stamp {
        Stamp {
            number,
            hash,
            clock: Some(self.clone()),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stamp {
    number: u64,
    hash: B256,
    #[serde(skip)]
    clock: Option<Clock>,
}
impl PartialEq for Stamp {
    fn eq(&self, other: &Self) -> bool {
        (self.number, self.hash) == (other.number, other.hash)
    }
}
impl Eq for Stamp {}
impl Stamp {
    pub fn current(&self) -> bool {
        self.clock.as_ref().is_some_and(|clock| {
            *clock.0.read().unwrap_or_else(|e| e.into_inner()) == Some((self.number, self.hash))
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub stamp: Stamp,
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
        self.stamp
            .current()
            .then_some(&self.levels[self.side(input)?][..])
    }
}
