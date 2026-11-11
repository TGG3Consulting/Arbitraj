//! Файл: core/op_error.rs
//! Что делает: ошибка операции ядра через шлюз (покупка, продажа, перевод) — текст для журнала и хронологии сделки
//! плюс код шлюза, если ошибку дала площадка. По коду исполнитель отличает «исход неизвестен» (OutcomeUnknown:
//! транзакция могла исполниться) от обычного отказа: такую сделку нельзя ни продавать, ни перегонять — только
//! ручная проверка (core/executor.rs).
//! Повтор продажи, не ушедшей в сеть, — retry_safe (решение 110) и rejected_unsent (решение 154), core/sell_unsent.rs.
//! Решение 229: продажа не попала в блок, а её nonce занят нашей отменой в блоке (квитанция отмены есть — исход
//! известен, исходная транзакция уже не исполнится) — cancel_confirmed, тоже повтор (core/sell_unsent.rs).
//! Повтор возврата USDT — withdraw_unsent (решение 163), core/usdt_return_retry.rs; решение 220: WithdrawRejected
//! («вывод точно не ушёл», но площадка откажет снова) там не повторяется — проверка в usdt_return_retry::fail.
//! Кто вызывает: core/trade_ops.rs, core/transfers.rs (создают), core/executor.rs, core/sell_unsent.rs (читают код).
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.

use crate::core::types::TxOutcome;
use crate::gateway::errors::{ErrorCode, GatewayError};

/// Ошибка операции: код шлюза (None — ошибка ядра, не площадки) и текст.
#[derive(Debug, Clone)]
pub struct OpError {
    pub code: Option<ErrorCode>,
    pub message: String,
    /// Транзакция или ордер уже ушли на площадку (у ошибки шлюза есть факт транзакции в сети). Решение 110.
    pub sent: bool,
    /// Решение 229: транзакция не попала в блок, а её nonce занят нашей отменой В БЛОКЕ (у факта транзакции исход
    /// NotIncluded и хеш отмены — адаптер ставит их только по квитанции отмены, venues/bsc_dex/send/track_util.rs
    /// landed). Исходная транзакция с этим nonce уже не исполнится.
    pub cancelled: bool,
}

impl OpError {
    /// Решение 238 (ревью, п. 2; дополнение — Тигран 09.11: согласовано): ошибка «не задана настройка или привязка»
    /// (код NotConfigured) с текстом. Принимает: текст. Ошибок нет.
    pub fn not_configured(message: String) -> Self {
        Self {
            code: Some(ErrorCode::NotConfigured),
            ..Self::from(message)
        }
    }

    /// Решение 238 (ревью, п. 2; дополнение — Тигран 09.11: согласовано): ошибка шлюза на пути котировки продажи —
    /// текстом `message`; код переносится, только если это NotConfigured (не задана настройка или привязка), прочие
    /// коды (сеть, нет стакана, устарело …) — без кода, как было (их вызывающие котировки не различают).
    /// Принимает: ошибку шлюза, текст. Ошибок нет.
    pub fn keep_not_configured(e: &GatewayError, message: String) -> Self {
        if e.code == ErrorCode::NotConfigured {
            Self::not_configured(message)
        } else {
            Self::from(message)
        }
    }

    /// Исход операции неизвестен (транзакция могла исполниться). Ошибок нет.
    pub fn outcome_unknown(&self) -> bool {
        self.code == Some(ErrorCode::OutcomeUnknown)
    }

    /// Решение 110: продажа DEX не ушла в сеть, и повтор со свежим маршрутом безопасен — денег и газа не стоил.
    /// Коды — только те, что возникают ДО отправки: цена сборки хуже допустимой, лимит или очередь запросов,
    /// нет маршрута, данные устарели, налоги не проверены, откат при пробном вызове (у отката в сети есть факт
    /// транзакции — sent), очередь кошелька остановлена (QueueHalted — транзакция не подписывалась, решение 231). Nonce, отказ канала, нет связи, исход неизвестен — не сюда: транзакция могла уйти.
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
                        // Решение 231 (дополнение): очередь кошелька остановлена — транзакция не подписывалась.
                        | ErrorCode::QueueHalted
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

    /// Решение 229 (Тигран, 08.11.2026: «риска нет: исход известен, деньги на месте»; живой случай JCT 08.11): продажа
    /// DEX не попала в блок за send.max_wait_blocks (TxNotIncluded), и nonce занят нашей отменой в блоке — исходная
    /// транзакция точно не исполнится, монеты на месте; повтор новой транзакцией безопасен. Отмены нет или её исход не
    /// подтверждён квитанцией (итог «исход неизвестен» — OutcomeUnknown, у бандла без отмены) — нет. Ошибок нет.
    pub fn cancel_confirmed(&self) -> bool {
        self.code == Some(ErrorCode::TxNotIncluded) && self.cancelled
    }

    /// Решение 163: прежний вывод (перегон USDT) точно не ушёл — повтор не создаст двойного вывода. Да: ошибка до
    /// вывода (своя, кода нет), площадка вывод отказала (WithdrawRejected; WithdrawLocked — удержание MEXC 16021, KuCoin
    /// 115008 и аналоги, решение 166; так же статус вывода «отказ» после создания:
    /// деньги вернулись на баланс), откат транзакции (SwapReverted) и отказы доступа, настроек, лимита запросов.
    /// Нет: исход неизвестен (адаптер уже искал вывод в истории по своему withdrawOrderId и не нашёл — MEXC), нет ответа
    /// или ответ не разобран (вывод мог создаться), у транзакции есть факт в сети. Ошибок нет.
    /// Решение 236, дополнение п. 4 (техлид 09.11): перевод из кошелька DEX не попал в блок, а его nonce занят нашей
    /// отменой в блоке (cancel_confirmed — обычный путь и позднее выяснение дают один код TxNotIncluded и один факт) —
    /// перевод не ушёл и уже не уйдёт, исход известен: да (как продажа в решении 229).
    pub fn withdraw_unsent(&self) -> bool {
        if self.cancel_confirmed() {
            return true;
        }
        if self.sent && self.code != Some(ErrorCode::SwapReverted) {
            return false;
        }
        match self.code {
            None => true,
            Some(c) => matches!(
                c,
                ErrorCode::WithdrawRejected
                    | ErrorCode::WithdrawLocked
                    | ErrorCode::InsufficientBalance
                    | ErrorCode::InvalidAddress
                    // Решение 253, дополнение t82: вывод отклонён из-за аккаунта — не создан.
                    | ErrorCode::AccountRestricted
                    | ErrorCode::SwapReverted
                    | ErrorCode::RateLimited
                    | ErrorCode::NotConfigured
                    | ErrorCode::SecretsLocked
                    | ErrorCode::VenueNotConnected
                    | ErrorCode::InvalidCredentials
                    | ErrorCode::InsufficientPermissions
                    | ErrorCode::IpNotAllowed
                    | ErrorCode::StaleData
                    // Решение 231 (дополнение): перевод из кошелька DEX не подписывался — очередь остановлена.
                    | ErrorCode::QueueHalted
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
            cancelled: false,
        }
    }
}

impl From<&GatewayError> for OpError {
    fn from(e: &GatewayError) -> Self {
        Self {
            code: Some(e.code),
            message: e.message.clone(),
            sent: e.tx.is_some(),
            cancelled: e
                .tx
                .as_ref()
                .is_some_and(|t| t.outcome == TxOutcome::NotIncluded && t.cancel_hash.is_some()),
        }
    }
}
