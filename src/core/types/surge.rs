//! Виртуальные резервы кривой Surge и доступные средства на одной высоте Ethereum.
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurgeState {
    pub token: Address,
    pub wrapped: Address,
    pub trader: Address,
    pub liquidity: U256,
    pub stock: U256,
    pub invariant: U256,
    pub cash: U256,
    /// Сколько SRG может принять выбранный покупатель по maxBag.
    pub capacity: U256,
    pub buy_mul: u8,
    pub sell_mul: u8,
}
