//! Файл: gateway/events.rs
//! Что делает: события, которые адаптеры шлют в шлюз, а шлюз — ядру и интерфейсу.
//! Кто вызывает: адаптеры (рыночные события), шлюз (события подключения и ошибок).
//! От чего зависит: core/types, gateway/errors.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::types::{OrderBook, Pool, VenueId};

use super::errors::GatewayError;

/// Рыночное событие от адаптера: новое состояние стакана или пула.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketEvent {
    /// Полный актуальный стакан монеты (адаптер сам собирает его из снимка и обновлений).
    Book(OrderBook),
    /// Актуальное состояние пула DEX.
    Pool(Pool),
    /// Площадка сообщила, что данные по монете больше недостоверны
    /// (разрыв последовательности, переподключение). Ядро считает данные устаревшими.
    Invalidated { venue: VenueId, ticker: String },
}

/// Состояние подключения площадки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Connected,
    Reconnecting,
    Disconnected,
}

/// Служебное событие шлюза: подключение или ошибка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GatewayEvent {
    Connection {
        venue: VenueId,
        state: ConnectionState,
        at: DateTime<Utc>,
    },
    Error {
        error: GatewayError,
        at: DateTime<Utc>,
    },
}
