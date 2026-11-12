//! Файл: tests/kraken_order_errors.rs
//! Что делает: решение 264 — строки ошибок Kraken из таблицы офдоки https://docs.kraken.com/api/docs/guides/spot-errors
//! (дословные строки ошибок): у ордера «EService:Deadline elapsed» («the order may have been placed despite the
//! timeout») и «EGeneral:Internal error[:5]» — «итог неизвестен» (поиск по cl_ord_id), не отказ; «EOrder:Domain rate
//! limit exceeded» — лимит (пауза), не отказ. Не у ордера смысл прежний.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::kraken::errors. Моков нет.

use arb::gateway::errors::ErrorCode;
use arb::venues::kraken::errors::{self, Op};

/// Единый код ошибки после того же пути, что у rest::private (from_errors → unsure).
fn code(line: &str, op: Op) -> ErrorCode {
    errors::unsure(errors::from_errors(&[line.to_string()], op, ""), op).code
}

#[test]
fn deadline_elapsed_on_order_is_outcome_unknown() {
    assert_eq!(
        code("EService:Deadline elapsed", Op::Trade),
        ErrorCode::OutcomeUnknown
    );
    assert_eq!(
        code("EService:Deadline elapsed", Op::Other),
        ErrorCode::BadResponse
    );
}

#[test]
fn internal_error_on_order_is_outcome_unknown() {
    assert_eq!(
        code("EGeneral:Internal error", Op::Trade),
        ErrorCode::OutcomeUnknown
    );
    assert_eq!(
        code("EGeneral:Internal error:5", Op::Trade),
        ErrorCode::OutcomeUnknown
    );
    assert_eq!(
        code("EGeneral:Internal error:5", Op::Other),
        ErrorCode::BadResponse
    );
}

#[test]
fn domain_rate_limit_is_rate_limit() {
    assert!(errors::is_rate_limit("EOrder:Domain rate limit exceeded"));
    assert_eq!(
        code("EOrder:Domain rate limit exceeded", Op::Trade),
        ErrorCode::RateLimited
    );
    // Прежние строки лимита не задеты.
    assert!(errors::is_rate_limit("EOrder:Rate limit exceeded"));
}
