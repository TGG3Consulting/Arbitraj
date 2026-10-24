//! Файл: venues/bsc_dex/node/jsonrpc.rs
//! Что делает: сборка запроса JSON-RPC 2.0 и разбор ответа (успех или ошибка {code, message, data})
//! в единые ошибки шлюза. Один код для HTTP и WebSocket узла и для каналов отправки.
//! Кто вызывает: node/rpc.rs, node/ws_task.rs, позже send/*.
//! От чего зависит: alloy::rpc::json_rpc (типы ответа JSON-RPC), errors.rs, serde_json.
//! Настройки: нет.
//!
//! Формат — спецификация JSON-RPC 2.0: https://www.jsonrpc.org/specification
//! (request object, response object, error object, batch); методы eth_* — execution-apis:
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth

use alloy::rpc::json_rpc::{Response, ResponsePayload};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::gateway::errors::GatewayResult;

use super::super::errors::{self, Op};

/// Тело одного запроса JSON-RPC 2.0.
/// Принимает: id, метод, параметры (массив). Возвращает: JSON-объект запроса. Ошибок нет.
pub fn request(id: u64, method: &str, params: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// Разбирает один объект ответа.
/// Принимает: объект ответа, источник (для текста ошибки), операцию, сырой текст ответа.
/// Возвращает: (id запроса, сырой result). Ошибки: ответ-ошибка узла (единый код), BadResponse.
pub fn parse(
    resp: Response,
    source: &str,
    op: Op,
    raw: &str,
) -> (Option<u64>, GatewayResult<Box<RawValue>>) {
    let id = match resp.id {
        alloy::rpc::json_rpc::Id::Number(n) => Some(n),
        _ => None,
    };
    let res = match resp.payload {
        ResponsePayload::Success(v) => Ok(v),
        ResponsePayload::Failure(e) => Err(errors::from_rpc(e.code, &e.message, source, op, raw)),
    };
    (id, res)
}

/// Разбирает текст ответа (один объект).
/// Принимает: текст, источник, операцию. Возвращает: сырой result.
/// Ошибки: не JSON-RPC (BadResponse), ответ-ошибка узла.
pub fn parse_text(text: &str, source: &str, op: Op) -> GatewayResult<Box<RawValue>> {
    let resp: Response = serde_json::from_str(text)
        .map_err(|e| errors::bad(&format!("не ответ JSON-RPC: {e}"), source, text))?;
    parse(resp, source, op, text).1
}

/// Приводит сырой result к нужному типу.
/// Принимает: сырой result, источник. Возвращает: значение. Ошибка BadResponse — другой вид.
pub fn decode<T: DeserializeOwned>(raw: &RawValue, source: &str) -> GatewayResult<T> {
    serde_json::from_str(raw.get())
        .map_err(|e| errors::bad(&format!("неожиданный вид result: {e}"), source, raw.get()))
}
