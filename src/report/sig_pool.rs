//! Файл: report/sig_pool.rs
//! Что делает: решение 255 — строка листа «Ликвидность пула» из состояния пула ядра (core/types/pool.rs Pool: вид,
//! семейство, ссылка на пул, номер блока, время получения, верхнее состояние — резервы V2 / Solidly, sqrtPriceX96 + тик +
//! ликвидность V3 / CL / Algebra, активный бин Bin и т. п.). Числа состояния U256 в JSON ядра — шестнадцатеричные
//! строки «0x…» (сериализация alloy-primitives); в отчёте — десятичные (адреса, 42 знака, и 32-байтовые id не трогаются).
//! Текст длиннее предела ячейки Excel — с явной пометкой «обрезано» (не молча).
//! Кто вызывает: report/sig_frames.rs (состояния в точках DEX окна), report/sig_episodes.rs (записи эпизодов).
//! От чего зависит: core/types (Pool, PoolRef), alloy-primitives (U256), sig_model.rs.
//! Настройки: нет.

use alloy_primitives::U256;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{Pool, PoolRef};

use super::sig_model::LiqRow;

/// Предел текста ячейки Excel — 32 767 символов («Total number of characters that a cell can contain»,
/// https://support.microsoft.com/office/excel-specifications-and-limits-1672b34d-7043-467e-8e27-269d656771c3).
pub const CELL_MAX_CHARS: usize = 32_767;

/// Что делает: строка листа из состояния пула. Принимает: ключ сигнала, источник, направление, пул. Ошибок нет.
pub fn pool_row(key: &str, source: &str, direction: &str, p: &Pool) -> LiqRow {
    LiqRow {
        key: key.into(),
        at: Some(p.received_at),
        source: source.into(),
        direction: direction.into(),
        venue: p.venue.0.clone(),
        pool: pool_ref(&p.reference),
        kind: format!("{:?} {}", p.kind, p.family),
        block: Some(p.block),
        state: fit(&state_text(&p.state)),
        ..Default::default()
    }
}

/// Ссылка на пул текстом — как в точке DEX окна (core/window_dex.rs pools). Ошибок нет.
pub fn pool_ref(r: &PoolRef) -> String {
    match r {
        PoolRef::Contract(a) => format!("{a:#x}"),
        PoolRef::Singleton { manager, id } => format!("{manager:#x}:{id:#x}"),
        PoolRef::Member { contract, coin } => format!("{contract:#x}:{coin:#x}"),
    }
}

/// Состояние пула JSON-текстом, числа «0x…» — десятичные. Ошибок нет (не сериализовалось — текст ошибки).
pub fn state_text<T: serde::Serialize>(s: &T) -> String {
    match serde_json::to_value(s) {
        Ok(mut v) => {
            dec(&mut v);
            v.to_string()
        }
        Err(e) => format!("состояние не прочитано: {e}"),
    }
}

/// Шестнадцатеричные числа → десятичные по всему JSON (кроме адресов и 32-байтовых id). Ошибок нет.
fn dec(v: &mut Value) {
    match v {
        Value::String(s) if s.starts_with("0x") && s.len() != 42 && s.len() < 66 => {
            if let Ok(n) = U256::from_str_radix(&s[2..], 16) {
                *s = n.to_string();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(dec),
        Value::Object(o) => o.values_mut().for_each(dec),
        _ => {}
    }
}

/// Текст в предел ячейки Excel с явной пометкой обрезки. Ошибок нет.
pub fn fit(s: &str) -> String {
    let n = s.chars().count();
    if n <= CELL_MAX_CHARS {
        return s.to_string();
    }
    let note = format!(" … [обрезано: предел ячейки Excel {CELL_MAX_CHARS} символов, всего {n}]");
    let keep = CELL_MAX_CHARS.saturating_sub(note.chars().count());
    s.chars().take(keep).collect::<String>() + &note
}

/// Время из JSON (RFC 3339). Ошибок нет: не время — None.
pub fn time_of(v: &Value) -> Option<DateTime<Utc>> {
    serde_json::from_value(v.clone()).ok()
}
