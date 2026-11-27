//! Общий контракт ручной проверки источника. Суммы в минимальных единицах — десятичные строки.
//! Пара определяется адресами. Проверки срока и специальные поля остаются в адаптере.
use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub input: Address,
    pub output: Address,
    #[serde(alias = "amount")]
    pub amount_raw: Option<String>,
    pub amount_tokens: Option<String>,
    /// Для Textile можно запросить точный выход вместо количества на входе.
    pub buy_amount_raw: Option<String>,
    pub trader: Option<Address>,
    pub market: Option<Address>,
    pub builder: Option<String>,
    pub firm: Option<FirmTerms>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirmTerms {
    pub wallet: String,
    pub minimum_raw: String,
    /// Максимальный расход для запроса Textile по точному выходу.
    pub maximum_raw: Option<String>,
    /// Сохранить проверенную заявку для обычного выбора торговым ядром по net.
    #[serde(default)]
    pub allow_trade: bool,
    pub fee_tenths_bps: Option<u32>,
}

#[derive(Serialize)]
pub struct Fee {
    pub asset: Address,
    pub amount_raw: String,
}

#[derive(Serialize)]
pub struct Preview {
    pub source: String,
    pub input: Address,
    pub output: Address,
    pub spent_raw: String,
    pub received_raw: String,
    pub unspent_raw: String,
    pub fee: Option<Fee>,
    pub block_hash: String,
    pub block: Option<u64>,
    pub expires: Option<u64>,
    /// Безопасные для вывода подробности проверки. Здесь нет ключей, подписи или готовой транзакции.
    pub details: serde_json::Value,
}
