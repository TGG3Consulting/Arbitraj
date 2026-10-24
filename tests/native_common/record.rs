//! Файл: tests/native_common/record.rs
//! Что делает: разбор записанных ответов узла (ответ по имени вызова, квитанция из итога вызова eth_simulateV1) и данных
//! вызовов (команды execute Universal Router, transfer ERC-20).
//! Кто вызывает: tests/bsc_dex_native_live.rs.
//! От чего зависит: arb::venues::bsc_dex::send::track::Receipt.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

use arb::venues::bsc_dex::exec::ur::IUniversalRouter;

use arb::venues::bsc_dex::send::track::Receipt;

/// Ответ записи по имени вызова. Паника — записи нет (тест на записанных данных).
pub fn answer<'a>(rec: &'a Value, name: &str) -> &'a Value {
    rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("нет ответа «{name}»"))
}

/// Квитанция из итога вызова симуляции: те же статус, газ и события (с data), что у eth_getTransactionReceipt.
pub fn receipt(call: &Value, block: &str) -> Receipt {
    serde_json::from_value(json!({
        "status": call["status"],
        "blockNumber": block,
        "gasUsed": call["gasUsed"],
        "effectiveGasPrice": "0x0",
        "logs": call["logs"],
    }))
    .unwrap()
}

/// Команды и входы execute. Ошибок нет.
pub fn decode(data: &Bytes) -> (Vec<u8>, Vec<Bytes>) {
    let call = IUniversalRouter::executeCall::abi_decode(data).unwrap();
    (call.commands.to_vec(), call.inputs)
}

/// Данные transfer ERC-20 (5 USDT на роутер перед покупкой). Ошибок нет.
pub fn usdt_to(router: Address, value: U256) -> Bytes {
    Bytes::from(
        arb::venues::bsc_dex::node::abi::IERC20::transferCall { to: router, value }.abi_encode(),
    )
}
