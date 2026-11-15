//! Файл: venues/bsc_dex/send/reject_log.rs
//! Что делает: решение 303, дополнение (журнал; живой случай SIREN 15.11.2026 20:49:06 UTC — все каналы отказали, а в
//! строках send.channel_rejected были только тексты «execution reverted» / «non-reverting tx in bundle failed», без
//! данных отката, без хеша транзакции и предела блока — причину установить не удалось). Текст строки журнала
//! send.channel_rejected по одному каналу: текст ошибки канала (с кодом JSON-RPC — как раньше), поле error.data ответа
//! канала, если оно есть (данные отката), расшифровка отката Permit2 (exec/permit2_err.rs), хеш транзакции и у бандла —
//! предел блока (maxBlockNumber / max_block_number), с которым он ушёл. Только журнал: на решения отправки не влияет.
//! Подписанная транзакция, ключи и адрес канала в строку не пишутся — только ответ канала (GatewayError::raw — тело
//! ответа, без секретов) и хеш.
//! Кто вызывает: send/mod.rs (classify).
//! От чего зависит: exec/permit2_err.rs, gateway/errors.rs (GatewayError), serde_json.
//! Настройки: нет.
//!
//! Поле data объекта ошибки — JSON-RPC 2.0, раздел 5.1 Error object (https://www.jsonrpc.org/specification):
//! «A Primitive or Structured value that contains additional information about the error. This may be omitted.»
//! Хеш транзакции = keccak256 её подписанного кодирования EIP-2718 (как send/absent.rs).

use alloy_primitives::{Bytes, hex, keccak256};
use serde_json::Value;

use crate::core::types::SendMethod;
use crate::gateway::errors::GatewayError;

use super::super::exec::permit2_err;

/// Строка журнала отказа одного канала. Принимает: id канала, его ошибку, отправленные транзакции (первая — сама
/// транзакция), способ отправки канала, предел блока бандла. Возвращает: текст строки. Ошибок нет.
pub fn line(
    id: &str,
    e: &GatewayError,
    txs: &[Bytes],
    method: SendMethod,
    max_block: u64,
) -> String {
    let data = error_data(e.raw.as_deref());
    let data_text = data.as_ref().map(|d| match d {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    let bytes = data_text
        .as_deref()
        .and_then(|s| s.strip_prefix("0x"))
        .and_then(|h| hex::decode(h).ok())
        .unwrap_or_default();
    let mut out = format!("{id}: {}", e.message);
    if let Some(d) = &data_text {
        out.push_str(&format!("; данные ошибки канала (error.data): {d}"));
    }
    if let Some(p2) = permit2_err::explain(&bytes, &e.message) {
        out.push_str(&format!(" — {p2}"));
    }
    if let Some(tx) = txs.first() {
        out.push_str(&format!("; транзакция {}", keccak256(tx)));
    }
    if method == SendMethod::Bundle {
        out.push_str(&format!(", бандл до блока {max_block}"));
    }
    out
}

/// Поле error.data ответа JSON-RPC канала. Принимает: сырой ответ (тело). Возвращает: значение (None — ответа нет,
/// не JSON или поля нет / null). Ошибок нет.
fn error_data(raw: Option<&str>) -> Option<Value> {
    let v: Value = serde_json::from_str(raw?).ok()?;
    v.get("error")?
        .get("data")
        .filter(|d| !d.is_null())
        .cloned()
}
