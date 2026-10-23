//! Файл: core/op_error.rs
//! Что делает: ошибка операции ядра через шлюз (покупка, продажа, перевод) — текст для журнала и хронологии сделки
//! плюс код шлюза, если ошибку дала площадка. По коду исполнитель отличает «исход неизвестен» (OutcomeUnknown:
//! транзакция могла исполниться) от обычного отказа: такую сделку нельзя ни продавать, ни перегонять — только
//! ручная проверка (core/executor.rs).
//! Кто вызывает: core/trade_ops.rs, core/transfers.rs (создают), core/executor.rs (читает код).
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.

use crate::gateway::errors::{ErrorCode, GatewayError};

/// Ошибка операции: код шлюза (None — ошибка ядра, не площадки) и текст.
#[derive(Debug, Clone)]
pub struct OpError {
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl OpError {
    /// Исход операции неизвестен (транзакция могла исполниться). Ошибок нет.
    pub fn outcome_unknown(&self) -> bool {
        self.code == Some(ErrorCode::OutcomeUnknown)
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for OpError {
    fn from(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }
}

impl From<&GatewayError> for OpError {
    fn from(e: &GatewayError) -> Self {
        Self {
            code: Some(e.code),
            message: e.message.clone(),
        }
    }
}
