//! Файл: venues/bsc_dex/node/head_stall_text.rs
//! Что делает: решение 282 — текст уведомлений «узел BSC: блок не растёт» (dex.node_stalled) и «блоки снова идут»
//! (dex.node_resumed): последний блок, время блока UTC (только если получено для этого же номера — не выдумывается),
//! когда бот увидел блок, сколько секунд стоит, ответ опроса, проверка свежести; при восстановлении — простой.
//! Кто вызывает: node/head_stall.rs (step).
//! От чего зависит: node/head_stall.rs (Change, SETTING), node/heads.rs (Head через Change), chrono.
//! Настройки: нет (порог — параметром).

use chrono::{TimeZone, Utc};

use super::head_stall::{Change, SETTING};

/// Время блока не известно для этого номера (кэш Heads — от другого блока или пуст).
const NO_TIME: &str =
    "не получено для этого блока (сразу берётся только при заданном feed.max_block_age_ms)";

/// Время в UTC для текста. Принимает: миллисекунды Unix. Возвращает: «ГГГГ-ММ-ДД чч:мм:сс.ммм UTC». Ошибок нет.
pub fn utc_ms(ms: i64) -> String {
    match Utc.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%Y-%m-%d %H:%M:%S%.3f UTC").to_string(),
        None => format!("{ms} мс Unix"),
    }
}

/// Текст сообщения (чистая, тестируется). Принимает: переход; порог (с); время блока (номер, мс Unix — Heads::
/// block_time_ms); последняя ошибка опроса; итог проверки свежести (None — свежо). Возвращает: текст. Ошибок нет.
pub fn text(
    c: &Change,
    limit_s: u64,
    block_time: Option<(u64, u64)>,
    poll_err: Option<&str>,
    fresh_err: Option<&str>,
) -> String {
    match c {
        Change::Stalled { head, stood_ms } => {
            let bt = block_time
                .filter(|(n, _)| *n == head.number)
                .and_then(|(_, ms)| i64::try_from(ms).ok())
                .map(utc_ms)
                .unwrap_or_else(|| NO_TIME.into());
            let poll = poll_err.map_or_else(
                || "узел отвечает, номер прежний".to_string(),
                |e| format!("ошибка — {e}"),
            );
            let fresh = fresh_err.unwrap_or("проходит");
            format!(
                "Последний блок: {}\nВремя блока: {bt}\nБот увидел этот блок: {}\nНомер не меняется: {} с (порог \
                 {SETTING} = {limit_s} с)\nОпрос eth_blockNumber: {poll}\nПроверка свежести головы: {fresh}\n\
                 Сообщение «блоки снова идут» придёт, когда номер вырастет; повторов нет.",
                head.number,
                utc_ms(head.received_at.timestamp_millis()),
                stood_ms / 1000,
            )
        }
        Change::Resumed { was, now, idle_ms } => format!(
            "Новый блок: {} (бот увидел: {})\nСтоял на блоке: {} (бот увидел: {})\nПростой: {} с",
            now.number,
            utc_ms(now.received_at.timestamp_millis()),
            was.number,
            utc_ms(was.received_at.timestamp_millis()),
            idle_ms / 1000,
        ),
    }
}
