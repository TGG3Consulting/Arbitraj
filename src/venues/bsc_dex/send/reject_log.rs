//! Файл: venues/bsc_dex/send/reject_log.rs
//! Что делает: решение 303, дополнение (журнал; живой случай SIREN 15.11.2026 20:49:06 UTC — все каналы отказали, а в
//! строках send.channel_rejected были только тексты «execution reverted» / «non-reverting tx in bundle failed», без
//! данных отката, без хеша транзакции и предела блока — причину установить не удалось). Текст строки журнала
//! send.channel_rejected по одному каналу: текст ошибки канала (с кодом JSON-RPC — как раньше), поле error.data ответа
//! канала, если оно есть (данные отката), расшифровка отката Permit2 (exec/permit2_err.rs), хеш транзакции и у бандла —
//! предел блока (maxBlockNumber / max_block_number), с которым он ушёл. Только журнал: на решения отправки не влияет.
//! Ревью 303: длина error.data в строке — не больше log.channel_data_max_bytes (не задана — целиком); Permit2
//! расшифровывается по ПОЛНЫМ данным до обрезки.
//! Подписанная транзакция, ключи и адрес канала в строку не пишутся — только ответ канала (GatewayError::raw — тело
//! ответа, без секретов) и хеш.
//! Кто вызывает: send/mod.rs (classify).
//! От чего зависит: exec/permit2_err.rs, gateway/errors.rs (GatewayError), ctx.rs, serde_json.
//! Настройки: log.channel_data_max_bytes (вся система, необязательная).
//!
//! Поле data объекта ошибки — JSON-RPC 2.0, раздел 5.1 Error object (https://www.jsonrpc.org/specification):
//! «A Primitive or Structured value that contains additional information about the error. This may be omitted.»
//! Хеш транзакции = keccak256 её подписанного кодирования EIP-2718 (как send/absent.rs).

use alloy_primitives::{Bytes, hex, keccak256};
use serde_json::Value;

use crate::core::types::SendMethod;
use crate::gateway::errors::GatewayError;

use super::super::ctx::Ctx;
use super::super::exec::permit2_err;

/// Предел длины error.data в строке журнала, байт (необязательная; не задана — данные целиком).
pub const DATA_MAX: &str = "log.channel_data_max_bytes";

/// Строка журнала отказа одного канала. Принимает: окружение (предел длины данных), id канала, его ошибку,
/// отправленные транзакции (первая — сама транзакция или первая транзакция бандла), способ отправки канала, предел
/// блока бандла. Возвращает: текст строки. Ошибок нет.
pub fn line(
    ctx: &Ctx,
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
        let max = ctx.int(DATA_MAX).ok().and_then(|n| usize::try_from(n).ok());
        out.push_str(&format!(
            "; данные ошибки канала (error.data): {}",
            cut(d, max)
        ));
    }
    // Расшифровка — по полным данным (bytes взяты до обрезки).
    if let Some(p2) = permit2_err::explain(&bytes, &e.message) {
        out.push_str(&format!(" — {p2}"));
    }
    // Хеш — только первой транзакции: у бандла число транзакций кодом не ограничено (разрешения + свопы цикла,
    // dispatch_slice; у одиночной сделки — обмен + перевод чаевых), а строка пишется на КАЖДЫЙ канал — все хеши
    // повторялись бы N раз. Первой транзакции и их числа хватает, чтобы найти бандл: у остальных тот же кошелёк и
    // следующие nonce. Одиночный канал получил только первую транзакцию (ready.rs send) — у него «транзакция».
    if let Some(tx) = txs.first() {
        let h = keccak256(tx);
        if method == SendMethod::Bundle && txs.len() > 1 {
            out.push_str(&format!(
                "; первая транзакция бандла {h} (всего {})",
                txs.len()
            ));
        } else {
            out.push_str(&format!("; транзакция {h}"));
        }
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

/// Данные не длиннее `max` байт (по границе символа) с пометкой «…(обрезано, всего N байт)»; None — целиком.
/// Ошибок нет.
fn cut(d: &str, max: Option<usize>) -> String {
    let Some(max) = max.filter(|m| *m < d.len()) else {
        return d.to_string();
    };
    let mut end = max;
    while !d.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…(обрезано, всего {} байт)", &d[..end], d.len())
}
