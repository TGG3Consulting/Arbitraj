//! Ручная проверка односетевой RFQ-котировки. Суммы и идентификаторы передаются без преобразования в float.
use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub trader: Address,
    pub input: Address,
    pub output: Address,
    pub amount: Option<String>,
    pub amount_tokens: Option<String>,
}

/// Только итог симуляции. Подпись маркетмейкера и calldata остаются внутри адаптера.
#[derive(Clone, Serialize)]
pub struct Preview {
    pub rfq_id: String,
    pub txid: String,
    pub amount_in: String,
    pub amount_out: String,
    pub amount_out_tokens: String,
    pub indicative_out: String,
    pub target: Address,
    pub selector: String,
    pub calldata_hash: String,
    pub value: String,
    pub expires: u64,
    pub block_hash: String,
    pub gas_used: String,
    pub maker_count: usize,
}
