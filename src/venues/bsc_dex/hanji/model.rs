//! Ручная котировка Hanji по контрактам токенов. Денежные суммы передаются целыми строками.
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Request {
    pub market: Option<Address>,
    pub input: Address,
    pub output: Address,
    pub amount_raw: String,
}

#[derive(Serialize)]
pub struct Preview {
    pub market: Address,
    pub block: u64,
    pub hash: B256,
    pub input: Address,
    pub output: Address,
    pub spent_raw: String,
    pub received_raw: String,
    pub unspent_raw: String,
    pub fee_asset: Address,
    pub fee_raw: String,
    pub last_price: String,
}
