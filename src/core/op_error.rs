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
    /// Транзакция или ордер уже ушли на площадку (у ошибки шлюза есть факт транзакции в сети). Решение 110.
    pub sent: bool,
}

impl OpError {
    /// Исход операции неизвестен (транзакция могла исполниться). Ошибок нет.
    pub fn outcome_unknown(&self) -> bool {
        self.code == Some(ErrorCode::OutcomeUnknown)
    }

    /// Решение 110: продажа DEX не ушла в сеть, и повтор со свежим маршрутом безопасен — денег и газа не стоил.
    /// Коды — только те, что возникают ДО отправки: цена сборки хуже допустимой, лимит или очередь запросов,
    /// нет маршрута, данные устарели, налоги не проверены, откат при пробном вызове (у отката в сети есть факт
    /// транзакции — sent). Nonce, отказ канала, нет связи, исход неизвестен — не сюда: транзакция могла уйти.
    pub fn retry_safe(&self) -> bool {
        !self.sent
            && matches!(
                self.code,
                Some(
                    ErrorCode::QuoteMismatch
                        | ErrorCode::RateLimited
                        | ErrorCode::NoRoute
                        | ErrorCode::StaleData
                        | ErrorCode::CoinNotChecked
                        | ErrorCode::SwapReverted
                )
            )
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
            sent: false,
        }
    }
}

impl From<&GatewayError> for OpError {
    fn from(e: &GatewayError) -> Self {
        Self {
            code: Some(e.code),
            message: e.message.clone(),
            sent: e.tx.is_some(),
        }
    }
}
