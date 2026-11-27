//! Две зарегистрированные валюты одной кассы DefiPlaza на каноническом блоке.
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

/// Единица расчёта поля oneMinusTradingFee, указанного как 0.64 в DeFiPlaza.sol:31.
/// <https://sourcify.dev/server/v2/contract/1/0xE68c1d72340aEeFe5Be76eDa63AE2f4bc7514110?fields=sources>
pub const MULTI_FEE_SCALE: u128 = 1u128 << 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefiPlazaState {
    pub pool: Address,
    pub module: Address,
    #[serde(default)]
    pub module_hash: B256,
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
