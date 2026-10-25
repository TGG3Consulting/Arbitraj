//! Файл: api/rules_codes.rs
//! Что делает: коды ошибок шлюза для формы правил (событие правила) — каждый код и его смысл по-русски, порядок как
//! в gateway/errors.rs; полнота перечня проверяется при сборке (сторож с исчерпывающим match).
//! Кто вызывает: api/rules_meta.rs.
//! От чего зависит: gateway/errors.rs (ErrorCode).
//! Настройки: нет.

use serde::Serialize;

use crate::gateway::errors::ErrorCode as C;

/// Код ошибки и его смысл по-русски (как в комментариях gateway/errors.rs).
#[derive(Serialize)]
pub struct CodeMeta {
    pub code: C,
    pub title: &'static str,
    /// «Повторить» для этого кода выбрать нельзя (gateway/rules.rs retry_allowed, решение 64).
    pub no_retry: bool,
}

/// Короткая запись кода. Принимает: код и смысл. Возвращает: CodeMeta.
fn c(code: C, title: &'static str) -> CodeMeta {
    let no_retry = !crate::gateway::rules::retry_allowed(code);
    CodeMeta {
        code,
        title,
        no_retry,
    }
}

/// Все коды ошибок — порядок как в gateway/errors.rs.
pub fn codes() -> Vec<CodeMeta> {
    vec![
        c(
            C::InvalidCredentials,
            "Ключ или подпись не приняты площадкой",
        ),
        c(
            C::InsufficientPermissions,
            "У ключа нет нужного права (торговля, вывод)",
        ),
        c(C::IpNotAllowed, "IP сервера не в белом списке ключа"),
        c(C::Timeout, "Площадка не ответила за отведённое время"),
        c(C::Unreachable, "Нет связи с площадкой (DNS, TCP, TLS)"),
        c(
            C::InvalidAddress,
            "Адрес отклонён (не в белом списке, неверный формат)",
        ),
        c(C::RateLimited, "Превышен лимит запросов площадки"),
        c(C::NotConfigured, "Нужная настройка не задана в интерфейсе"),
        c(C::SecretsLocked, "Секреты не разблокированы паролем"),
        c(C::InsufficientBalance, "Не хватает средств"),
        c(C::OrderRejected, "Ордер отклонён площадкой"),
        c(C::OrderNotFilled, "Ордер отправлен, но не исполнился"),
        c(C::PartialFill, "Ордер исполнился частично"),
        c(C::WithdrawRejected, "Вывод отклонён площадкой"),
        c(
            C::DepositNotCredited,
            "Депозит не зачислен за отведённое время",
        ),
        c(C::SwapReverted, "Транзакция свопа в сети откатилась"),
        c(C::StaleData, "Данные устарели — торговать нельзя"),
        c(C::VenueNotConnected, "Адаптер площадки не подключён"),
        c(C::BadResponse, "Ответ площадки не удалось разобрать"),
        c(C::NotApplicable, "Функция к этой площадке не применима"),
        c(
            C::CoinNotChecked,
            "Налог монеты или крючок ещё не проверены",
        ),
        c(C::CoinCheckFailed, "Проверка монеты или пула не прошла"),
        c(C::NoRoute, "Нет допустимого пула или маршрута"),
        c(
            C::QuoteMismatch,
            "Проверка перед отправкой разошлась с расчётом",
        ),
        c(C::ChannelRejected, "Канал отправки не принял транзакцию"),
        c(C::GasBelowChannelMin, "Цена газа ниже минимума канала"),
        c(C::TxNotIncluded, "Транзакция не попала в блок вовремя"),
        c(C::NonceConflict, "Nonce неверный или занят"),
        c(
            C::OutcomeUnknown,
            "Результат неизвестен: ордер, вывод или транзакция отправлены, итог не получен — не повторяется никогда, сверка и ручная проверка",
        ),
        c(C::Internal, "Внутренняя ошибка нашего кода"),
    ]
}

/// Сторож полноты кодов: новый код в gateway/errors.rs → ошибка сборки → добавить в codes().
const _: fn(C) = |code| match code {
    C::InvalidCredentials | C::InsufficientPermissions | C::IpNotAllowed | C::Timeout => {}
    C::Unreachable | C::InvalidAddress | C::RateLimited | C::NotConfigured => {}
    C::SecretsLocked | C::InsufficientBalance | C::OrderRejected | C::OrderNotFilled => {}
    C::PartialFill | C::WithdrawRejected | C::DepositNotCredited | C::SwapReverted => {}
    C::StaleData | C::VenueNotConnected | C::BadResponse | C::Internal => {}
    C::NotApplicable | C::CoinNotChecked | C::CoinCheckFailed | C::NoRoute => {}
    C::QuoteMismatch | C::ChannelRejected | C::GasBelowChannelMin | C::TxNotIncluded => {}
    C::NonceConflict => {}
    C::OutcomeUnknown => {}
};
