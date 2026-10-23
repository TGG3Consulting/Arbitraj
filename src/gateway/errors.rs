//! Файл: gateway/errors.rs
//! Что делает: единые коды ошибок шлюза. Каждый адаптер переводит ошибки своей площадки
//! в эти коды; ядро и правила шлюза работают только с ними.
//! Кто вызывает: адаптеры (создают), шлюз и ядро (обрабатывают), интерфейс (показывает).
//! От чего зависит: core/types (VenueId, ChainTx).
//! Настройки: нет.
//!
//! Это стык: новый код добавляется только через запрос на изменение стыка (docs/AGENTS.md).

use serde::{Deserialize, Serialize};

use crate::core::types::{ChainTx, VenueId};

/// Код ошибки шлюза. По нему правила шлюза решают, что делать дальше.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    /// Ключ или подпись не приняты площадкой.
    InvalidCredentials,
    /// У ключа нет нужного права (торговля, вывод).
    InsufficientPermissions,
    /// IP сервера не в белом списке ключа.
    IpNotAllowed,
    /// Площадка не ответила за отведённое время.
    Timeout,
    /// Нет связи с площадкой (DNS, TCP, TLS).
    Unreachable,
    /// Адрес отклонён (не в белом списке, неверный формат).
    InvalidAddress,
    /// Превышен лимит запросов площадки.
    RateLimited,
    /// Нужная настройка не задана в интерфейсе.
    NotConfigured,
    /// Секреты не разблокированы паролем.
    SecretsLocked,
    /// Не хватает средств.
    InsufficientBalance,
    /// Ордер отклонён площадкой.
    OrderRejected,
    /// Ордер отправлен, но не исполнился.
    OrderNotFilled,
    /// Ордер исполнился частично.
    PartialFill,
    /// Вывод отклонён площадкой.
    WithdrawRejected,
    /// Депозит не зачислен за отведённое время.
    DepositNotCredited,
    /// Транзакция свопа в сети откатилась.
    SwapReverted,
    /// Данные устарели — торговать нельзя.
    StaleData,
    /// Адаптер площадки не подключён.
    VenueNotConnected,
    /// Ответ площадки не удалось разобрать.
    BadResponse,
    /// Функция контракта к этой площадке не применима (например, проверка налогов токена у биржи).
    NotApplicable,
    /// Налог монеты или влияние крючка ещё не проверены — торговли нет (решения 30, 32).
    CoinNotChecked,
    /// Проверка монеты или пула не прошла (пробный обмен откатился, узел вернул ошибку) — чёрный список.
    CoinCheckFailed,
    /// Нет допустимого пула или маршрута для монеты (выключены, ликвидность ниже порога, крючок не допущен).
    NoRoute,
    /// Проверка перед отправкой (квотер / eth_call) разошлась с расчётом больше допуска — сделки нет.
    QuoteMismatch,
    /// Канал отправки не принял транзакцию (ответ-ошибка канала, лимит запросов канала).
    ChannelRejected,
    /// Цена газа ниже минимума канала (настройка или ответ канала «require GasPrice»).
    GasBelowChannelMin,
    /// Транзакция не попала в блок за отведённое время (deadline / send.max_wait_blocks).
    TxNotIncluded,
    /// Nonce неверный или занят другой транзакцией (nonce too low, вытеснена).
    NonceConflict,
    /// Внутренняя ошибка нашего кода.
    Internal,
}

/// Ошибка шлюза: код, площадка, понятное сообщение, сырой ответ площадки и — если транзакция
/// уже ушла в сеть — её факт (газ потрачен даже при ошибке, решение 36).
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?} ({venue:?}): {message}")]
pub struct GatewayError {
    pub code: ErrorCode,
    /// Площадка; None — ошибка не связана с конкретной площадкой.
    pub venue: Option<VenueId>,
    /// Понятная причина по-русски — её видит Тигран в интерфейсе и Telegram.
    pub message: String,
    /// Сырой ответ площадки (без секретов) для разбора.
    pub raw: Option<String>,
    /// Транзакция в сети, если ошибка случилась после отправки (откат, не попала в блок).
    /// В коробке, чтобы ошибка оставалась маленькой (она возвращается из множества функций).
    pub tx: Option<Box<ChainTx>>,
}

impl GatewayError {
    /// Создаёт ошибку.
    /// Принимает: код, площадку (или None), сообщение.
    /// Возвращает: GatewayError без сырого ответа.
    pub fn new(code: ErrorCode, venue: Option<VenueId>, message: impl Into<String>) -> Self {
        Self {
            code,
            venue,
            message: message.into(),
            raw: None,
            tx: None,
        }
    }

    /// Добавляет факт транзакции в сети.
    /// Принимает: транзакцию. Возвращает: ту же ошибку с полем tx.
    pub fn with_tx(mut self, tx: ChainTx) -> Self {
        self.tx = Some(Box::new(tx));
        self
    }

    /// Добавляет сырой ответ площадки.
    /// Принимает: текст ответа. Возвращает: ту же ошибку с полем raw.
    pub fn with_raw(mut self, raw: impl Into<String>) -> Self {
        self.raw = Some(raw.into());
        self
    }
}

/// Результат операций шлюза.
pub type GatewayResult<T> = Result<T, GatewayError>;
