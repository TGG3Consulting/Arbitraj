//! Две зарегистрированные валюты одной кассы DefiPlaza на каноническом блоке.
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefiPlazaState {
    pub pool: Address,
    pub module: Address,
    pub tokens: [Address; 2],
    pub physical: [Address; 2],
    /// Балансы ERC20/native для Multi, ячейки нормализованного учета для Stable.
    pub reserves: [U256; 2],
    pub balances: [U256; 2],
    pub factors: [U256; 2],
    pub curve: DefiPlazaCurve,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DefiPlazaCurve {
    Multi { one_minus_fee: u64 },
    Stable { fee_bps: u8, delta: u64 },
}
