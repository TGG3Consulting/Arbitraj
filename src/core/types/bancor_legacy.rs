//! Денежный путь конвертера Bancor 2.1. Нормализованный WETH и физический ETH сохраняются отдельно.
use super::LadderPoint;
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BancorLegacyBridge {
    pub middle: Address,
    pub anchor: Address,
    pub converter: Address,
    pub fee_ppm: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BancorLegacyState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<BancorLegacyBridge>,
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
    pub fn path(&self, input: Address) -> Option<Vec<Address>> {
        let i = self.side(input)?;
        let mut path = vec![self.reserves[0], self.anchor];
        if let Some(s) = &self.second {
            path.extend([s.middle, s.anchor]);
        }
        path.push(self.reserves[1]);
        if i == 1 {
            path.reverse();
        }
        Some(path)
    }
    pub fn points(&self, input: Address) -> Option<&[LadderPoint]> {
        Some(&self.levels[self.side(input)?])
    }
}
