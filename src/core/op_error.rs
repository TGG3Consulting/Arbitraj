//! Файл: core/op_error.rs
//! Что делает: ошибка операции ядра через шлюз (покупка, продажа, перевод) — текст для журнала и хронологии сделки
//! плюс код шлюза, если ошибку дала площадка. По коду исполнитель отличает «исход неизвестен» (OutcomeUnknown:
//! транзакция могла исполниться) от обычного отказа: такую сделку нельзя ни продавать, ни перегонять — только
//! ручная проверка (core/executor.rs).
//! Повтор продажи, не ушедшей в сеть, — retry_safe (решение 110) и rejected_unsent (решение 154), core/sell_unsent.rs.
//! Повтор возврата USDT — withdraw_unsent (решение 163), core/usdt_return_retry.rs.
//! Кто вызывает: core/trade_ops.rs, core/transfers.rs (создают), core/executor.rs, core/sell_unsent.rs (читают код).
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

    /// Решение 154: ни один канал отправки не принял транзакцию, и адаптер своим узлом проверил, что её нет ни в блоке,
    /// ни в пуле (venues/bsc_dex/send/absent.rs: ChannelRejected без факта транзакции означает «точно не ушла»; узел её
    /// знает или проверка не удалась — адаптер ChannelRejected не отдаёт, транзакция идёт к трекеру). Повтор новой
    /// транзакцией безопасен. Ошибок нет.
    pub fn rejected_unsent(&self) -> bool {
        !self.sent && self.code == Some(ErrorCode::ChannelRejected)
    }

    /// Решение 163: прежний вывод (перегон USDT) точно не ушёл — повтор не создаст двойного вывода. Да: ошибка до
    /// вывода (своя, кода нет), площадка вывод отказала (WithdrawRejected — в том числе удержание MEXC 16021; так же статус вывода «отказ» после создания:
    /// деньги вернулись на баланс), откат транзакции (SwapReverted) и отказы доступа, настроек, лимита запросов.
    /// Нет: исход неизвестен (адаптер уже искал вывод в истории по своему withdrawOrderId и не нашёл — MEXC), нет ответа
    /// или ответ не разобран (вывод мог создаться), у транзакции есть факт в сети. Ошибок нет.
    pub fn withdraw_unsent(&self) -> bool {
        if self.sent && self.code != Some(ErrorCode::SwapReverted) {
            return false;
        }
        match self.code {
            None => true,
            Some(c) => matches!(
                c,
                ErrorCode::WithdrawRejected
                    | ErrorCode::InsufficientBalance
                    | ErrorCode::InvalidAddress
                    | ErrorCode::SwapReverted
                    | ErrorCode::RateLimited
                    | ErrorCode::NotConfigured
                    | ErrorCode::SecretsLocked
                    | ErrorCode::VenueNotConnected
                    | ErrorCode::InvalidCredentials
                    | ErrorCode::InsufficientPermissions
                    | ErrorCode::IpNotAllowed
                    | ErrorCode::StaleData
            ),
        }
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
