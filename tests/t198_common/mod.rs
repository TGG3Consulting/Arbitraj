//! Файл: tests/t198_common/mod.rs
//! Что делает: решение 413 (t198) — общие помощники тестов распределения и факта: числа, записанный журнал живого
//! случая WKC → MEXC 18.11 (tests/data/wkc_mexc_2026-11-18/journal.json), ждущая доли, входы распределения.
//! Кто вызывает: tests/core_credit_split_t198.rs, tests/core_credit_fact_t198.rs.
//! От чего зависит: arb::core::credit_split, serde_json, rust_decimal.

#![allow(dead_code)]

use arb::core::credit_split::{Input, Pending};
use rust_decimal::Decimal;
use serde_json::Value;

/// Число из строки. Ошибок нет (тест падает).
pub fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Записанный журнал живого случая.
pub fn journal() -> Value {
    let p = format!(
        "{}/tests/data/wkc_mexc_2026-11-18/journal.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Число журнала по пути JSON Pointer.
pub fn j(v: &Value, ptr: &str) -> Decimal {
    d(v.pointer(ptr).and_then(Value::as_str).unwrap())
}

/// Ждущая доли (снимок без времени).
pub fn pend(id: &str, record: Decimal, outsider: Option<Decimal>) -> Pending {
    Pending {
        id: id.into(),
        record,
        outsider,
        seen: None,
    }
}

/// Входы без удержания и допуска.
pub fn input(fact: Decimal, confirmed: Decimal, pending: Vec<Pending>) -> Input {
    Input {
        fact,
        confirmed,
        pending,
        unclear: false,
        hold_pct: Decimal::ZERO,
        tol_pct: Decimal::ZERO,
    }
}
