//! Денежный путь конвертера Bancor 2.1. Нормализованный WETH и физический ETH сохраняются отдельно.
use super::LadderPoint;
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BancorLegacyState {
    pub anchor: Address,
    pub converter: Address,
    pub network: Address,
    pub tokens: [Address; 2],
    pub reserves: [Address; 2],
    pub balances: [U256; 2],
    pub wallet_balances: [U256; 2],
    pub fee_ppm: u32,
    pub levels: [Vec<LadderPoint>; 2],
}
impl BancorLegacyState {
    fn side(&self, input: Address) -> Option<usize> {
        self.tokens.iter().position(|t| *t == input)
    }
    pub fn path(&self, input: Address) -> Option<[Address; 3]> {
        let i = self.side(input)?;
        Some([self.reserves[i], self.anchor, self.reserves[1 - i]])
    }
    pub fn points(&self, input: Address) -> Option<&[LadderPoint]> {
        Some(&self.levels[self.side(input)?])
    }
}
