//! Ручная котировка Hanji по контрактам токенов. Денежные суммы передаются целыми строками.
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Request {
    pub market: Option<Address>,
    pub input: Address,
    pub output: Address,
    pub amount_raw: Option<String>,
    pub amount_tokens: Option<String>,
    pub trader: Option<Address>,
}

#[derive(Serialize)]
pub struct Preview {
    /// Оценка публичной книги. Вызов от другого контракта может получить другую ликвидность.
    pub execution_checked: bool,
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
    pub payer: Option<Address>,
    pub executor: Option<Address>,
    pub gas_used: Option<u64>,
    pub deadline: Option<u64>,
}
