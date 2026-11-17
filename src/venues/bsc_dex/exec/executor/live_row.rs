//! Файл: venues/bsc_dex/exec/executor/live_row.rs
//! Что делает: решение 400 (t185) — разбор строки summary.json записи симуляции исполнителя v3 на живых пулах
//! (вынесено из live_rule.rs: файл был больше 200 строк). Формат строки — заметка
//! docs/notes/t181-executor-all-kinds-2026-11-17.md, п. 5, и README записи: case («a» — покупка (а), «b» — продажа (б),
//! «c» — сплит (в), «g_cycle» — цикл (г), «нет пулов»), kind (19 / 20 / 21), pool_id, hooks, status (0 / 1), got,
//! quoter_out (квотер на H), quoter_out_after_a (у (б): квотер в той же симуляции сразу после (а)), got_eq_swap_credit,
//! x_left (остатки на исполнителе), revert; необязательное `"stable": true` — пул Infinity Stable.
//! Кто вызывает: exec/executor/live_rule.rs.
//! От чего зависит: serde_json, alloy-primitives, exec/executor/live_rule.rs (Class, Hook).
//! Настройки: нет.

use alloy_primitives::{Address, U256};
use serde_json::Value;

use super::live_rule::{Class, Hook};

/// Строка summary.json, нужная правилу (format — заметка t181, п. 5, и README записи).
pub struct Row {
    pub case: String,
    pub class: Class,
    pub pool: String,
    pub ok: bool,
    pub revert: String,
    pub x_zero: bool,
    pub credit: bool,
    pub got: Option<U256>,
    pub quoter: Option<U256>,
}

/// Разбор строки вызова summary.json. Ошибки (текст): нет поля, статус не 0/1, число не десятичной строкой.
pub fn row(v: &Value) -> Result<Row, String> {
    let case = text(v, "case")?;
    if !["a", "b", "c", "g_cycle"].contains(&case.as_str()) {
        return Err(format!("summary.json: неизвестный case «{case}»"));
    }
    let kind = v["kind"]
        .as_u64()
        .and_then(|k| u8::try_from(k).ok())
        .ok_or_else(|| format!("summary.json: у строки {case} нет вида kind"))?;
    let hooks: Address = text(v, "hooks")?
        .parse()
        .map_err(|e| format!("summary.json: hooks: {e}"))?;
    let hook = if v["stable"].as_bool() == Some(true) {
        Hook::Stable
    } else if hooks == Address::ZERO {
        Hook::Without
    } else {
        Hook::With
    };
    let ok = match v["status"].as_u64() {
        Some(1) => true,
        Some(0) => false,
        _ => return Err(format!("summary.json: статус строки {case} не 0 / 1")),
    };
    let x_zero = v["x_left"].as_object().is_some_and(|m| {
        m.values()
            .all(|x| x.as_str() == Some("0") || x.as_u64() == Some(0))
    });
    // (б) — квотер той же симуляции после (а); остальные — quoter_out (у (а) — на H, то же состояние).
    let quoter = if case == "b" {
        "quoter_out_after_a"
    } else {
        "quoter_out"
    };
    Ok(Row {
        class: Class { kind, hook },
        pool: text(v, "pool_id")?.to_ascii_lowercase(),
        ok,
        revert: v["revert"].as_str().unwrap_or_default().to_string(),
        x_zero,
        credit: v["got_eq_swap_credit"].as_bool() == Some(true),
        got: num(&v["got"])?,
        quoter: num(&v[quoter])?,
        case,
    })
}

/// Строковое поле. Ошибки (текст): нет поля или не строка.
pub fn text(v: &Value, k: &str) -> Result<String, String> {
    v[k].as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("запись: нет строкового поля {k}"))
}

/// Число десятичной строкой или null. Ошибки (текст): иное.
fn num(v: &Value) -> Result<Option<U256>, String> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => U256::from_str_radix(s, 10)
            .map(Some)
            .map_err(|e| format!("запись: число «{s}»: {e}")),
        other => Err(format!("запись: число строкой ожидалось, пришло {other}")),
    }
}
