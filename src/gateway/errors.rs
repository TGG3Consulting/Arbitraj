//! Файл: gateway/errors.rs
//! Что делает: единые коды ошибок шлюза. Каждый адаптер переводит ошибки своей площадки
//! в эти коды; ядро и правила шлюза работают только с ними.
//! Кто вызывает: адаптеры (создают), шлюз и ядро (обрабатывают), интерфейс (показывает).
//! От чего зависит: core/types (VenueId).
//! Настройки: нет.
//!
//! Это стык: новый код добавляется только через запрос на изменение стыка (docs/AGENTS.md).

use serde::{Deserialize, Serialize};

use crate::core::types::VenueId;

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
    /// Внутренняя ошибка нашего кода.
    Internal,
}

/// Ошибка шлюза: код, площадка, понятное сообщение и сырой ответ площадки.
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
        }
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
